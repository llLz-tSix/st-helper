const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window.getCurrentWindow();

const modifier = /Mac/.test(navigator.platform) ? "Cmd+Shift" : "Ctrl+Shift";
for (const button of document.querySelectorAll("#bar button")) {
  button.title = button.title.replace("Ctrl+Shift", modifier);
}

const $ = (id) => document.getElementById(id);
const dot = $("dot"), statusEl = $("status"), modeEl = $("mode");
const questionEl = $("question"), answerEl = $("answer");
let currentId = 0;

function setStatus({ state, text }) {
  dot.className = state;
  statusEl.className = state === "error" ? "error" : "";
  statusEl.textContent = text;
  statusEl.title = text;
  $("btn-pause").textContent = state === "paused" ? "▶" : "❚❚";
}

function applyUi({ opacity, font_size }) {
  document.documentElement.style.setProperty("--bg-alpha", opacity);
  document.documentElement.style.setProperty("--font", font_size + "px");
}

function setClickThrough(on) {
  modeEl.textContent = on ? "сквозной" : "";
  document.body.classList.toggle("passthrough", on);
}

function appendText(text) {
  const span = document.createElement("span");
  span.className = "fade";
  span.textContent = text;
  answerEl.appendChild(span);
}

listen("status", (e) => setStatus(e.payload));
listen("ui-settings", (e) => applyUi(e.payload));
listen("click-through", (e) => setClickThrough(e.payload));

listen("question", (e) => {
  currentId = e.payload.id;
  questionEl.textContent = e.payload.text;
  answerEl.textContent = "";
  answerEl.scrollTop = 0;
});

listen("answer-delta", (e) => {
  if (e.payload.id !== currentId) return;
  const nearBottom = answerEl.scrollHeight - answerEl.scrollTop - answerEl.clientHeight < 30;
  appendText(e.payload.text);
  if (nearBottom) answerEl.scrollTop = answerEl.scrollHeight;
});

listen("clear", () => {
  currentId = -1;
  questionEl.textContent = "";
  answerEl.innerHTML = '<span class="hint">Ожидаю вопрос…</span>';
});

listen("scroll", (e) => {
  answerEl.scrollBy({ top: e.payload * answerEl.clientHeight * 0.6 });
});

$("btn-settings").onclick = () => invoke("open_settings");
$("btn-pause").onclick = () => invoke("toggle_pause");
$("btn-hide").onclick = () => win.hide();
$("grip").addEventListener("mousedown", (e) => {
  e.preventDefault();
  win.startResizeDragging("SouthEast");
});

invoke("get_state").then((s) => {
  setStatus(s.status);
  applyUi(s);
  setClickThrough(s.click_through);
});
