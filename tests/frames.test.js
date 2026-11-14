import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs';
import {acceptFull,paint} from '../web/frames.js';
const demo=()=>JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
test('accept actual Rust engine fixture and isolate external mutation',()=>{const raw=demo(),f=acceptFull(null,raw);raw.rows[0][0].text='X';assert.notEqual(f.rows[0][0].text,'X');assert.equal(f.cols,64);});
test('reject foreign session/epoch, stale revisions and invalid sizes atomically',()=>{const old=acceptFull(null,demo());for(const change of [{session:'other'},{epoch:2},{revision:0},{revision:1},{revision:2,cols:0},{revision:2,cursor_col:999}]){assert.throws(()=>acceptFull(old,{...demo(),...change}));assert.equal(old.revision,1);}});
test('reject malformed cells and non-advanced layout resize',()=>{for(const text of ['\x1b','xx','']){const f=demo();f.rows[0][0].text=text;assert.throws(()=>acceptFull(null,f));}const f=demo();f.revision=2;f.rows.pop();assert.throws(()=>acceptFull(acceptFull(null,demo()),f));});
test('paint uses cell-local canvas operations, no terminal interpretation',()=>{let texts=0;const g={scale(){},fillRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){texts++},restore(){},strokeRect(){}};const canvas={style:{},getContext:()=>g};paint(canvas,acceptFull(null,demo()),2);assert.equal(texts,512);assert.equal(canvas.width,1536);});

test('Rust-owned hidden cursor is not painted',()=>{let cursors=0;const g={scale(){},fillRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(){cursors++}};const canvas={style:{},getContext:()=>g};paint(canvas,acceptFull(null,{...demo(),cursor_visible:false}));assert.equal(cursors,0);assert.throws(()=>acceptFull(null,{...demo(),cursor_visible:0}));});

test('reject control and unpaired scalar cells; allow astral Unicode',()=>{for(const text of ['\u0085','\ud800','\udfff']){const f=demo();f.rows[0][0].text=text;assert.throws(()=>acceptFull(null,f));}const f=demo();f.rows[0][0].text='😀';assert.equal(acceptFull(null,f).rows[0][0].text,'😀');});
test('canvas scale is bounded even with hostile or invalid device scale',()=>{const g={scale(x){assert.equal(x,1)},fillRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(){}};for(const scale of [Infinity,NaN,-1,1000]){const canvas={style:{},getContext:()=>g};paint(canvas,acceptFull(null,demo()),scale);assert.equal(canvas.width,768);}});
test('largest permitted grid stays under 16 megapixel backing allocation',()=>{const g={scale(){},fillRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(){}};const raw=demo();raw.cols=256;raw.rows=Array.from({length:128},()=>Array.from({length:256},()=>({text:' ',foreground:'#ffffff',background:'#000000',width:1,style:0})));const canvas={style:{},getContext:()=>g};paint(canvas,acceptFull(null,raw),4);assert.ok(canvas.width*canvas.height<=16777216);});
test('actual Rust Unicode frame keeps two-column paint and styled colors',()=>{
 const raw=JSON.parse(fs.readFileSync(new URL('../web/unicode-frame.json',import.meta.url),'utf8'));
 const f=acceptFull(null,raw,{session:'unicode-demo',epoch:1});
 assert.equal(f.rows[0][0].text,'中');assert.equal(f.rows[0][0].width,2);assert.equal(f.rows[0][1].width,0);
 assert.equal(f.rows[0][0].foreground,'#123456');assert.equal(f.rows[0][0].background,'#ff0000');assert.equal(f.rows[0][0].style,15);
 assert.equal(f.rows[1][0].foreground,'#2472c8');assert.equal(f.rows[1][0].background,'#cd3131');
 const calls=[];const g={scale(){},fillRect(...args){calls.push(['background-or-decoration',...args]);},save(){},beginPath(){},rect(...args){calls.push(['clip',...args]);},clip(){},fillText(text){calls.push(['glyph',text,this.font,this.globalAlpha]);},restore(){},strokeRect(){}};
 paint({style:{},getContext:()=>g},f);
 const firstGlyph=calls.findIndex(c=>c[0]==='glyph');assert.ok(firstGlyph>=4*24);
 assert.deepEqual(calls.find(c=>c[0]==='clip'),['clip',0,0,24,24]);
 assert.deepEqual(calls.find(c=>c[0]==='glyph'),['glyph','中','italic bold 18px monospace',1]);
 assert.equal(calls.filter(c=>c[0]==='glyph'&&c[1]==='中').length,1);
 assert.ok(calls.some(c=>c[0]==='glyph'&&c[1]==='D'&&c[3]===0.5));
});
test('reject inconsistent wide-pair geometry atomically',()=>{for(const mutate of [f=>f.rows[0][0].width=0,f=>f.rows[0][0].width=2,f=>f.rows[0][63].width=2,f=>f.rows[0][0].style=32]){const f=demo();mutate(f);assert.throws(()=>acceptFull(null,f));}});
test('Rust cursor shapes paint at exact cell geometry and invalid shapes reject',()=>{const raw=demo();raw.cursor_row=2;raw.cursor_col=3;for(const shape of ['block','beam','underline']){const fills=[],strokes=[];const g={scale(){},fillRect(...x){fills.push(x)},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(...x){strokes.push(x)}};paint({style:{},getContext:()=>g},acceptFull(null,{...raw,cursor_shape:shape}));if(shape==='block')assert.deepEqual(strokes.at(-1),[36.5,48.5,11,23]);if(shape==='beam')assert.deepEqual(fills.at(-1),[36,48,2,24]);if(shape==='underline')assert.deepEqual(fills.at(-1),[36,70,12,2]);}assert.throws(()=>acceptFull(null,{...raw,cursor_shape:'invalid'}));assert.throws(()=>acceptFull(null,{...raw,pending_wrap:1}));});
