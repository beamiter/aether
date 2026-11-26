import assert from 'node:assert/strict';
import {test} from 'node:test';
import {acceptsRun, readNotebookFile, copyNotebookStream} from '../web/notebook-panel.js';

const selected = (name, bytes, size = bytes.length) => ({
  name, size, arrayBuffer: async () => Uint8Array.from(bytes).buffer,
});

test('notebook selection keeps exact UTF-8 and checks the actual bytes', async () => {
  const content = '\ufeff# Notebook\n中\n';
  assert.equal(await readNotebookFile(selected('READ.JTNB.MD', new TextEncoder().encode(content))), content);
  await assert.rejects(readNotebookFile(selected('read.md', [])), /jtnb/);
  await assert.rejects(readNotebookFile(selected('read.jtnb.md', [0xff])), /UTF-8/);
  await assert.rejects(readNotebookFile(selected('read.jtnb.md', new Uint8Array(1024 * 1024 + 1), 0)), /1 MiB/);
});

test('a partial or stopped receipt can only update the exact current notebook cell run', () => {
  const previous = {documentId: 'd', runId: 'r', cell: 1, revision: 1};
  assert.equal(acceptsRun(previous, 'd', null, 1), true);
  assert.equal(acceptsRun({...previous, revision: 2}, 'd', previous), true);
  assert.equal(acceptsRun({...previous, revision: 2}, 'other', previous), false);
  assert.equal(acceptsRun({...previous, revision: 2, cell: 2}, 'd', previous), false);
  assert.equal(acceptsRun({...previous, revision: 2, runId: 'retired'}, 'd', previous), false);
  assert.equal(acceptsRun(previous, 'd', previous), false);
});


test('native notebook copy preserves exact lane bytes including trailing LF and Unicode', async () => {
  const snapshot = {stdout: 'line α\n\n', stderr: 'err\tβ\n'};
  const calls = [];
  const invoke = async (...args) => calls.push(args);
  await copyNotebookStream(snapshot, 'stdout', invoke);
  await copyNotebookStream(snapshot, 'stderr', invoke);
  assert.deepEqual(calls, [['copy_terminal_text', {text: snapshot.stdout}], ['copy_terminal_text', {text: snapshot.stderr}]]);
});

test('native notebook copy refuses missing or mismatched lanes and propagates native failure', async () => {
  let invoked = false;
  for (const [snapshot, lane] of [[null, 'stdout'], [{stdout:'a'}, 'both'], [{stderr:null}, 'stderr']]) {
    await assert.rejects(copyNotebookStream(snapshot, lane, async () => { invoked = true; }));
  }
  assert.equal(invoked, false);
  await assert.rejects(copyNotebookStream({stdout:''}, 'stdout', async () => {throw Error('clipboard unavailable');}), /clipboard unavailable/);
});
