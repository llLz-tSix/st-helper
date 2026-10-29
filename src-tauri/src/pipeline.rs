//! Utterance -> transcription -> question check -> streamed answer.

use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::audio::Utterance;
use crate::settings::Settings;
use crate::{base_status, set_status, Shared};

const HISTORY_LEN: usize = 6;
/// The model answers with this marker when the utterance needs no answer.
const SKIP_MARK: &str = "NO_ANSWER";

pub async fn run(app: AppHandle, shared: Arc<Shared>, mut rx: UnboundedReceiver<Utterance>) {
    while let Some(utt) = rx.recv().await {
        if shared.paused.load(Ordering::Relaxed) {
            continue;
        }
        let (key, lang) = {
            let s = shared.settings.lock().unwrap();
            (s.groq_key.clone(), s.stt_language.clone())
        };
        if key.is_empty() {
            set_status(&app, &shared, "error", "Укажите ключ Groq в настройках (Ctrl+Shift+S)");
            continue;
        }
        set_status(&app, &shared, "transcribing", "Распознаю…");
        let t0 = std::time::Instant::now();
        let res = crate::groq::transcribe(&shared.http, &key, crate::groq::wav_bytes(&utt.pcm), &lang).await;
        #[cfg(debug_assertions)]
        eprintln!("[stt] {} ms, {} chars", t0.elapsed().as_millis(), res.as_ref().map_or(0, |t| t.chars().count()));
        let _ = t0;
        if shared.paused.load(Ordering::Relaxed) {
            base_status(&app, &shared); // paused while the request was in flight
            continue;
        }
        let text = match res {
            Ok(t) => t,
            Err(e) => {
                set_status(&app, &shared, "error", &e.message());
                continue;
            }
        };
        if is_noise(&text) {
            base_status(&app, &shared);
            continue;
        }
        let _ = app.emit("heard", json!({ "text": text }));
        push_history(&shared, "user", format!("Собеседник: {text}"));
        // Short remarks without question cues are skipped without asking the LLM;
        // everything else goes straight to generation, where the prompt itself
        // filters non-questions (no extra round trip).
        if !looks_like_question(&text) && text.split_whitespace().count() < 4 {
            base_status(&app, &shared);
            continue;
        }
        let id = shared.gen_id.fetch_add(1, Ordering::SeqCst) + 1;
        tauri::async_runtime::spawn(generate(app.clone(), shared.clone(), id, text));
    }
}

fn push_history(shared: &Shared, role: &str, content: String) {
    let mut h = shared.history.lock().unwrap();
    h.push_back((role.to_string(), content));
    while h.len() > HISTORY_LEN {
        h.pop_front();
    }
}

async fn generate(app: AppHandle, shared: Arc<Shared>, id: u64, question: String) {
    shared.inflight.fetch_add(1, Ordering::SeqCst);
    set_status(&app, &shared, "generating", "Генерирую ответ…");

    let (key, model, system) = {
        let s = shared.settings.lock().unwrap();
        (s.groq_key.clone(), s.model.clone(), system_prompt(&s))
    };
    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system })];
    for (role, content) in shared.history.lock().unwrap().iter() {
        messages.push(json!({ "role": role, "content": content }));
    }

    let t0 = std::time::Instant::now();
    let mut pending = String::new();
    let mut gated = true;
    let mut answer = String::new();
    let mut preempted = false;
    let res = crate::groq::chat_stream(&shared.http, &key, &model, messages, |delta| {
        // A newer question (or a clear) took over: stop this one.
        if shared.active_id.load(Ordering::SeqCst) > id {
            preempted = true;
            return false;
        }
        let out = if gated {
            pending.push_str(delta);
            let t = pending.trim_start();
            if t.starts_with(SKIP_MARK) {
                return false;
            }
            if SKIP_MARK.starts_with(t) {
                return true; // could still turn into the marker
            }
            gated = false;
            #[cfg(debug_assertions)]
            eprintln!("[llm] first text after {} ms", t0.elapsed().as_millis());
            // Claim the answer area; an earlier question can't override a later one.
            if shared.active_id.fetch_max(id, Ordering::SeqCst) > id {
                preempted = true;
                return false;
            }
            let _ = app.emit("question", json!({ "id": id, "text": question }));
            t.to_string()
        } else {
            delta.to_string()
        };
        answer.push_str(&out);
        let _ = app.emit("answer-delta", json!({ "id": id, "text": out }));
        true
    })
    .await;

    let _ = t0;
    #[cfg(debug_assertions)]
    eprintln!("[llm] done after {} ms, skipped={}, preempted={}", t0.elapsed().as_millis(), gated, preempted);
    shared.inflight.fetch_sub(1, Ordering::SeqCst);
    match res {
        Err(e) if !preempted => set_status(&app, &shared, "error", &e.message()),
        _ => {
            if !gated && !preempted {
                let _ = app.emit("answer-done", json!({ "id": id }));
                push_history(&shared, "assistant", answer.trim().to_string());
            }
            base_status(&app, &shared);
        }
    }
}

fn system_prompt(s: &Settings) -> String {
    let or_none = |t: &str| if t.trim().is_empty() { "(не указано)".to_string() } else { t.trim().to_string() };
    format!(
        "Ты — незаметный подсказчик кандидата на собеседовании. Ты пишешь текст, который кандидат прочитает вслух.\n\
         Язык ответа: {lang}.\n\
         Стиль: {style}.\n\n\
         Резюме кандидата:\n<<<\n{resume}\n>>>\n\n\
         Описание вакансии:\n<<<\n{vacancy}\n>>>\n\n\
         Правила ответа:\n\
         - Говори от первого лица, как будто отвечает сам кандидат.\n\
         - Коротко: 2–4 предложения ИЛИ 3–5 коротких пунктов (каждый с новой строки, начинается с «- »). Весь ответ читается вслух за 20–30 секунд.\n\
         - Поведенческие вопросы (опыт, проекты, конфликты, достижения, ошибки): опирайся только на факты из резюме. Не придумывай истории, ситуации, проекты, компании, роли, цифры и результаты, которых нет в резюме. Если резюме пустое или подходящего эпизода в нём нет — не сочиняй случай, а честно и нейтрально опиши свой подход в общем виде («В таких ситуациях я обычно…»).\n\
         - Технические вопросы: сначала суть ответа, затем короткий пример или пояснение.\n\
         - Без вступлений («Отличный вопрос», «Конечно» и т.п.), без повторения вопроса, без markdown-выделения (**, #, `).\n\
         - Реплики собеседника получены распознаванием речи и могут содержать ошибки — восстанавливай смысл по контексту.\n\
         - Если последняя реплика собеседника — не вопрос и не просьба что-то рассказать, объяснить или сделать (приветствие, комментарий, согласие, рассказ о компании, фраза не по делу), ответь ровно одним словом: {SKIP_MARK}",
        lang = or_none(&s.answer_language),
        style = or_none(&s.style),
        resume = or_none(&s.resume),
        vacancy = or_none(&s.vacancy),
    )
}

fn looks_like_question(t: &str) -> bool {
    if t.contains('?') {
        return true;
    }
    const CUES: &[&str] = &[
        "как ", "что ", "почему", "зачем", "расскаж", "опиши", "объясн", "какой", "какая", "какие",
        "каким", "каков", "где ", "когда", "сколько", "можете", "могли бы", "чем ", "привед", "поделит",
        "назови", "перечисл", "в чём", "в чем", "разниц", "отлича", "ли ", "давайте", "представьт",
        "what", "how", "why", "tell me", "describe", "explain", "can you", "could you", "walk me",
        "which", "when", "where", "do you", "have you", "would you", "difference",
    ];
    let l = format!("{} ", t.to_lowercase());
    CUES.iter().any(|c| l.contains(c))
}

/// Empty results and typical Whisper hallucinations on music/silence.
fn is_noise(t: &str) -> bool {
    let l = t.to_lowercase();
    if l.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
        return true;
    }
    const HALLUCINATIONS: &[&str] = &[
        "продолжение следует", "субтитры", "спасибо за просмотр", "редактор субтитров", "dimatorzok",
        "подписывайтесь на канал", "thanks for watching", "thank you for watching", "amara.org",
    ];
    l.chars().count() < 80 && HALLUCINATIONS.iter().any(|h| l.contains(h))
}
