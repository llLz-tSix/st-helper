const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);
const TEXT = ["model", "stt_language", "answer_language", "style", "resume", "vacancy"];
const NUM = ["vad_threshold", "pause_ms", "min_utterance_ms", "max_utterance_s", "opacity", "font_size"];
const FMT = {
  vad_threshold: (v) => Number(v).toFixed(3),
  pause_ms: (v) => (v / 1000).toFixed(1) + " с",
  opacity: (v) => Math.round(v * 100) + "%",
  font_size: (v) => v + " px",
};
// Level meter on a -60..0 dBFS scale.
const meterPos = (v) => Math.max(0, Math.min(100, ((20 * Math.log10(Math.max(v, 1e-6)) + 60) / 60) * 100));
let current = {};
let msgTimer;

function msg(text, err = false) {
  $("msg").textContent = text;
  $("msg").className = err ? "err" : "";
  clearTimeout(msgTimer);
  if (!err) msgTimer = setTimeout(() => ($("msg").textContent = ""), 3000);
}

function updateOutputs() {
  for (const k of Object.keys(FMT)) $(k + "_v").textContent = FMT[k]($(k).value);
  $("thr").style.left = meterPos($("vad_threshold").value) + "%";
}

async function load() {
  current = await invoke("get_settings");
  for (const k of TEXT) $(k).value = current[k] ?? "";
  for (const k of NUM) $(k).value = current[k];
  $("use_mic").checked = !!current.use_mic;
  $("key").value = "";
  $("key").placeholder = current.has_key ? "ключ сохранён — оставьте пустым, чтобы не менять" : "gsk_…";
  $("key-note").textContent = current.has_key
    ? "Ключ хранится локально и не показывается."
    : "Бесплатный ключ: console.groq.com → API Keys.";
  updateOutputs();
}

async function save() {
  const s = { ...current };
  delete s.has_key;
  for (const k of TEXT) s[k] = $(k).value;
  for (const k of NUM) s[k] = Number($(k).value);
  s.use_mic = $("use_mic").checked;
  s.groq_key = "";
  try {
    await invoke("save_settings", { settings: s, newKey: $("key").value || null });
    await load();
    msg("Сохранено");
  } catch (e) {
    msg("Ошибка сохранения: " + e, true);
  }
}

for (const k of Object.keys(FMT)) $(k).addEventListener("input", updateOutputs);
$("save").onclick = save;
$("close").onclick = () => win.hide();
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") win.hide();
  if (e.key === "s" && e.ctrlKey && !e.shiftKey) { e.preventDefault(); save(); }
});

listen("settings-open", load);
listen("level", (e) => {
  if (e.payload.source !== "system") return;
  const pos = meterPos(e.payload.level);
  $("lvl").style.width = pos + "%";
  $("lvl").style.background = pos >= meterPos($("vad_threshold").value) ? "#3ecf6e" : "#4a5060";
});

load();
