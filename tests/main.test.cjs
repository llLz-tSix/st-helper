const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { test } = require('node:test');
const vm = require('node:vm');

function setup() {
  const listeners = new Map();
  const frames = new Map();
  const elements = new Map();
  let nextFrame = 0;
  const element = (id) => {
    if (!elements.has(id)) elements.set(id, {
      textContent: '', children: [], scrollHeight: 100, scrollTop: 0, clientHeight: 100,
      appendChild(child) { this.children.push(child); },
      addEventListener() {},
    });
    return elements.get(id);
  };
  vm.runInNewContext(readFileSync(require.resolve('../src/main.js'), 'utf8'), {
    window: { __TAURI__: {
      core: { invoke: () => new Promise(() => {}) },
      event: { listen: (name, cb) => listeners.set(name, cb) },
      window: { getCurrentWindow: () => ({}) },
    } },
    navigator: { platform: 'Win32' },
    document: {
      getElementById: element, querySelectorAll: () => [],
      createElement: () => ({ textContent: '' }),
    },
    requestAnimationFrame(cb) { frames.set(++nextFrame, cb); return nextFrame; },
    cancelAnimationFrame(id) { frames.delete(id); },
  });
  return {
    answer: element('answer'), frames,
    emit: (name, payload) => listeners.get(name)({ payload }),
    render() {
      const callbacks = [...frames.values()];
      frames.clear();
      for (const cb of callbacks) cb();
    },
  };
}

test('stream fragments share one render and retain their order', () => {
  const ui = setup();
  ui.emit('question', { id: 1, text: 'Question' });
  ui.emit('answer-delta', { id: 1, text: 'Hello ' });
  ui.emit('answer-delta', { id: 1, text: 'world' });
  assert.equal(ui.frames.size, 1);
  assert.equal(ui.answer.children.length, 0);
  ui.render();
  assert.equal(ui.answer.children[0].textContent, 'Hello world');
  assert.equal(ui.answer.scrollTop, 100);
});

test('clear and replacement questions discard pending text and old deltas', () => {
  const ui = setup();
  ui.emit('question', { id: 1, text: 'First' });
  ui.emit('answer-delta', { id: 1, text: 'discard' });
  ui.emit('clear');
  ui.render();
  assert.equal(ui.answer.children.length, 0);
  ui.emit('question', { id: 2, text: 'Second' });
  ui.emit('answer-delta', { id: 2, text: 'also discard' });
  ui.emit('question', { id: 3, text: 'Third' });
  ui.emit('answer-delta', { id: 2, text: 'late delta' });
  ui.emit('answer-delta', { id: 3, text: 'keep' });
  ui.render();
  assert.deepEqual(ui.answer.children.map((c) => c.textContent), ['keep']);
});

test('rendering leaves scroll position alone when reading earlier text', () => {
  const ui = setup();
  ui.emit('question', { id: 1, text: 'Question' });
  ui.answer.scrollHeight = 500;
  ui.answer.scrollTop = 80;
  ui.emit('answer-delta', { id: 1, text: 'next' });
  ui.render();
  assert.equal(ui.answer.scrollTop, 80);
});
