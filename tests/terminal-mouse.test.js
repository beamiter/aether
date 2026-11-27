import test from 'node:test';
import assert from 'node:assert/strict';
import {installTerminalMouse} from '../web/terminal-mouse.js';

function harness(tracking = 'drag', dispatch = () => {}, retainRelease = false) {
  const listeners = new Map(), sent = [], globals = new Map();
  let bounds = {left:10, top:20, width:120, height:160};
  let frame = {epoch:1, layout:7, revision:11, mouse:{tracking, generation:8}, cols:6, rows:Array(8),
    session:'pty-demo'};
  let capture = null, epoch = 1, focused = 0, available = true, releaseAllowed = retainRelease;
  const attachment = {};
  frame.attachment = attachment;
  const canvas = {
    addEventListener(type, listener, options) {listeners.set(type, {listener, options});},
    getBoundingClientRect() {return bounds;},
    setPointerCapture(id) {capture = id;},
    hasPointerCapture(id) {return capture === id;},
    releasePointerCapture() {capture = null;},
  };
  const previousAdd = globalThis.addEventListener;
  globalThis.addEventListener = (kind, fn) => globals.set(kind, fn);
  const input = installTerminalMouse({
    canvas, getFrame:() => available && frame.epoch === epoch ? frame : null,
    canRelease: painted => releaseAllowed && painted.attachment === attachment && frame.epoch === epoch,
    send(report) {sent.push(report); return dispatch(report);},
    focus() {focused++;},
  });
  if (previousAdd) globalThis.addEventListener = previousAdd;
  else delete globalThis.addEventListener;
  return {
    setBounds(next) {bounds = {...bounds, ...next}; input.frameChanged();},
    global(type, event={}) {globals.get(type)?.(event);},
    sent, focused:() => focused, capture:() => capture,
    setFrame(next) {frame = {...frame, ...next}; input.frameChanged();},
    setEpoch(next) {epoch = next; input.frameChanged();},
    setAvailable(next) {available = next; input.frameChanged();},
    setReleaseAllowed(next) {releaseAllowed = next; input.frameChanged();},
    event(type, data = {}) {
      const event = {isPrimary:true, pointerType:'mouse', pointerId:1, button:0,
        clientX:49, clientY:61, deltaX:0, deltaY:0, deltaMode:1, buttons:0,
        preventDefault() {this.defaultPrevented = true;}, ...data};
      listeners.get(type).listener(event);
      return Boolean(event.defaultPrevented);
    },
  };
}

test('application drag maps physical cells and releases the captured button outside the canvas', () => {
  const h = harness();
  assert.equal(h.event('pointerdown', {button:2, ctrlKey:true}), true);
  assert.equal(h.capture(), 1);
  assert.equal(h.focused(), 1);
  h.event('pointermove', {clientX:131, clientY:21, altKey:true});
  h.event('pointerup', {clientX:400, clientY:-4});
  assert.equal(h.event('contextmenu'), true);
  assert.equal(h.capture(), null);
  assert.deepEqual(h.sent.map(({kind, row, col, button, alt, ctrl}) => ({kind, row, col, button, alt, ctrl})), [
    {kind:'press', row:2, col:1, button:'right', alt:false, ctrl:true},
    {kind:'motion', row:0, col:5, button:'right', alt:true, ctrl:false},
    {kind:'release', row:0, col:5, button:'right', alt:false, ctrl:false},
  ]);
  assert.ok(h.sent.every(report => report.layout === '7' && report.revision === '11' && report.generation === 8));
});

test('release follows the current revision when the application repaints during a drag', () => {
  const h = harness();
  h.event('pointerdown');
  h.setFrame({revision:12});
  h.event('pointerup');
  assert.deepEqual(h.sent.map(({kind, revision}) => [kind, revision]), [
    ['press', '11'], ['release', '12'],
  ]);
});

test('alternate-scroll route sends stamped wheel events and leaves pointer input local', async () => {
  const h = harness('alternate_scroll');
  assert.equal(h.event('pointerdown'), false);
  assert.equal(h.event('pointermove'), false);
  assert.equal(h.event('click'), false);
  assert.equal(h.event('contextmenu'), false);
  assert.equal(h.event('wheel', {deltaY: -1, shiftKey: true}), false);
  assert.equal(h.event('wheel', {deltaX: 20, deltaY: 1}), false);
  assert.equal(h.event('wheel', {deltaY: -1}), true);
  await Promise.resolve();
  assert.equal(h.event('wheel', {deltaY: 1}), true);
  assert.equal(h.capture(), null);
  assert.equal(h.focused(), 0);
  assert.deepEqual(h.sent.map(({kind, revision, generation, button}) => [kind, revision, generation, button]), [
    ['wheel_up', '11', 8, null],
    ['wheel_down', '11', 8, null],
  ]);
});

test('Shift and non-mouse pointers stay local while vertical wheel pulses obey tracking ownership', () => {
  const h = harness('button');
  assert.equal(h.event('pointerdown', {shiftKey:true}), false);
  assert.equal(h.event('click', {shiftKey:true, ctrlKey:true}), false);
  assert.equal(h.event('pointerdown', {pointerType:'touch'}), false);
  assert.equal(h.event('wheel', {deltaX:20, deltaY:1}), false);
  assert.equal(h.event('wheel', {deltaY:20, shiftKey:true}), false);
  assert.equal(h.event('wheel', {deltaY:-20, ctrlKey:true}), true);
  assert.deepEqual(h.sent, [{layout:'7', revision:'11', generation:8, kind:'wheel_up',
    row:2, col:1, button:null, alt:false, ctrl:true}]);
});

test('hover is coalesced and stale pending reports cannot reach a new layout, mode, or epoch', async () => {
  let releaseFirst;
  const first = new Promise(resolve => {releaseFirst = resolve;});
  const h = harness('any', () => first);
  h.event('pointermove', {clientX:40});
  h.event('pointermove', {clientX:60});
  h.event('pointermove', {clientX:100});
  h.setFrame({layout:9});
  releaseFirst();
  await first;
  assert.equal(h.sent.length, 1);
  assert.equal(h.sent[0].layout, '7');
  h.event('pointerdown');
  h.setFrame({mouse:{tracking:'off', generation:10}});
  h.event('pointerup');
  assert.equal(h.capture(), null);
  h.setEpoch(2);
  h.event('pointerdown');
  assert.deepEqual(h.sent.map(report => report.kind), ['motion', 'press']);
});

test('transient publication hold retains drag and releases with the original surface stamp',()=>{
 for(const kind of ['pointerup','pointercancel','lostpointercapture']){
  let held=false;
  const admitted=[];
  const h=harness('drag',report=>{
   if(report.kind==='press'){if(held)return false;held=true;}
   if(report.kind==='release')held=false;
   admitted.push(report.kind);
   return true;
  },true);
  h.event('pointerdown');
  h.setAvailable(false);
  assert.equal(h.capture(),1);
  h.event('pointermove',{clientX:100});
  h.event(kind,{clientX:400,clientY:-10});
  assert.deepEqual(admitted,['press','release']);
  assert.equal(held,false);
  assert.equal(h.sent[1].revision,'11');
  assert.equal(h.sent[1].layout,'7');
  assert.equal(h.sent[1].generation,8);
  assert.equal(h.capture(),null);
  h.setAvailable(true);
  h.event('pointerdown');
  assert.deepEqual(admitted,['press','release','press']);
 }
});

test('attachment retirement and new surface ownership suppress a held release fallback',()=>{
 for(const retire of [
  h=>h.setReleaseAllowed(false),
  h=>h.setEpoch(2),
  h=>h.setFrame({attachment:{}}),
  h=>h.setFrame({layout:8}),
  h=>h.setFrame({mouse:{tracking:'button',generation:9}}),
 ]){
  const h=harness('drag',()=>{},true);
  h.event('pointerdown');
  h.setAvailable(false);
  retire(h);
  h.setAvailable(true);
  h.event('pointerup');
  assert.deepEqual(h.sent.map(report=>report.kind),['press']);
  assert.equal(h.capture(),null);
 }
});

test('press-only tracking sends independent presses without capture, release, or motion',()=>{
 const h=harness('press_only');
 assert.equal(h.event('pointerdown'),true);
 assert.equal(h.capture(),null);
 h.event('pointermove',{buttons:1});
 assert.equal(h.event('pointerup'),false);
 h.event('pointercancel');
 assert.equal(h.event('click'),true);
 h.event('pointerdown',{button:2,altKey:true,ctrlKey:true});
 h.event('pointerup',{button:2});
 h.event('lostpointercapture');
 assert.equal(h.event('contextmenu'),true);
 assert.deepEqual(h.sent.map(report=>[report.kind,report.button,report.alt,report.ctrl]),[
  ['press','left',false,false],['press','right',true,true],
 ]);
 assert.equal(h.focused(),2);
 assert.equal(h.capture(),null);
});

test('press-only tracking leaves wheel and explicit local overrides available locally',()=>{
 const h=harness('press_only');
 assert.equal(h.event('wheel',{deltaY:20}),false);
 assert.equal(h.event('wheel',{deltaY:-20}),false);
 assert.equal(h.event('pointerdown',{shiftKey:true}),false);
 assert.equal(h.event('pointerdown',{metaKey:true}),false);
 assert.equal(h.event('pointerdown',{getModifierState:key=>key==='AltGraph'}),false);
 assert.deepEqual(h.sent,[]);
});

async function settleWheel() { for (let i=0;i<80;i++) await Promise.resolve(); }

test('pixel wheel noise is fractional while line and page events use row units', async()=>{
 const h=harness('button');
 assert.equal(h.event('wheel',{deltaMode:0,deltaY:-.25}),true);
 h.event('wheel',{deltaMode:0,deltaY:-.25});
 await settleWheel();assert.equal(h.sent.length,0);
 h.event('wheel',{deltaMode:0,deltaY:-19.5});await settleWheel();
 assert.equal(h.sent.length,1);
 h.event('wheel',{deltaMode:1,deltaY:2});await settleWheel();
 assert.deepEqual(h.sent.slice(1).map(x=>x.kind),['wheel_down','wheel_down']);
 const page=harness('button');page.event('wheel',{deltaMode:2,deltaY:-1});await settleWheel();
 assert.equal(page.sent.length,8);
});

test('wheel fraction binding survives repaint but not direction, modifier or local gaps',async()=>{
 for(const gap of [h=>h.event('wheel',{deltaY:.1}),h=>h.event('wheel',{deltaY:-.1,altKey:true}),
  h=>h.event('wheel',{deltaY:-.1,ctrlKey:true}),h=>h.event('wheel',{deltaY:-.1,shiftKey:true}),
  h=>h.event('wheel',{deltaY:NaN}),h=>h.event('wheel',{deltaY:-1,clientX:999}),
  h=>h.event('pointerleave'),h=>h.global('blur'),h=>h.global('keydown',{key:'Alt'})]){
  const h=harness('button');h.event('wheel',{deltaY:-.6});gap(h);h.event('wheel',{deltaY:-.5});
  await settleWheel();assert.equal(h.sent.length,0);
 }
 const h=harness('button');h.event('wheel',{deltaY:-.6});h.setFrame({revision:12});
 h.event('wheel',{deltaY:-.5});await settleWheel();assert.equal(h.sent.length,1);assert.equal(h.sent[0].revision,'12');
});

test('owner and CSS geometry changes retire fractional wheel motion',async()=>{
 for(const change of [h=>h.setFrame({attachment:{}}),h=>h.setFrame({layout:8}),
  h=>h.setFrame({mouse:{tracking:'button',generation:9}}),h=>h.setBounds({height:180}),
  h=>h.setBounds({top:21}),h=>{h.setAvailable(false);h.setAvailable(true);}]){
  const h=harness('button');h.event('wheel',{deltaY:-.6});change(h);h.event('wheel',{deltaY:-.5});
  await settleWheel();assert.equal(h.sent.length,0);
 }
});

test('bounded wheel FIFO preserves directions and exact modifiers behind accepted IPC',async()=>{
 let done;const first=new Promise(resolve=>done=resolve);let calls=0;
 const h=harness('any',()=>++calls===1?first:undefined);
 h.event('pointermove'); // accepted motion is a barrier; wheels cannot overtake it
 h.event('wheel',{deltaY:-2});h.event('wheel',{deltaY:1,altKey:true});h.event('wheel',{deltaY:-1,ctrlKey:true});
 assert.deepEqual(h.sent.map(x=>x.kind),['motion']);
 done();await settleWheel();
 assert.deepEqual(h.sent.slice(1).map(x=>[x.kind,x.alt,x.ctrl]),[
  ['wheel_up',false,false],['wheel_up',false,false],['wheel_down',true,false],['wheel_up',false,true],
 ]);
});

test('whole wheel pulses stay ordered across pointer/modifier gaps while partials do not count',async()=>{
 let done;const first=new Promise(resolve=>done=resolve);let calls=0;
 const h=harness('button',()=>++calls===1?first:undefined);
 h.event('wheel',{deltaY:-1});h.event('wheel',{deltaY:-.6});
 h.event('pointerleave');h.event('wheel',{deltaY:1,altKey:true});h.global('blur');
 h.event('wheel',{deltaY:-.5});
 done();await settleWheel();assert.deepEqual(h.sent.map(x=>[x.kind,x.alt]),[['wheel_up',false],['wheel_down',true]]);
});

test('wheel outstanding budget is 64 whole pulses total and no fractional event is credited',async()=>{
 let done;const first=new Promise(resolve=>done=resolve);let calls=0;
 const h=harness('button',()=>++calls===1?first:undefined);
 h.event('wheel',{deltaY:-100000});h.event('wheel',{deltaY:-100000});
 assert.equal(h.sent.length,1);done();await settleWheel();assert.equal(h.sent.length,64);
 h.event('wheel',{deltaY:-.4});await settleWheel();assert.equal(h.sent.length,64);
});

test('stale wheel queues are dropped while a new owner still waits for accepted flight',async()=>{
 let done;const first=new Promise(resolve=>done=resolve);let calls=0;
 const h=harness('button',()=>++calls===1?first:undefined);
 h.event('wheel',{deltaY:-3});h.setFrame({attachment:{},layout:8});
 h.event('wheel',{deltaY:1,altKey:true});assert.equal(h.sent.length,1);
 done();await settleWheel();assert.equal(h.sent.length,2);
 assert.deepEqual([h.sent[1].kind,h.sent[1].layout,h.sent[1].alt],['wheel_down','8',true]);
});

test('repaint-only queue rebases to the current visible revision and geometry invalidation drops old whole pulses',async()=>{
 for(const resize of [false,true]){
  let done;const first=new Promise(resolve=>done=resolve);let calls=0;
  const h=harness('button',()=>++calls===1?first:undefined);
  h.event('wheel',{deltaY:-3});h.setFrame({revision:12});if(resize)h.setBounds({width:180});
  done();await settleWheel();assert.equal(h.sent.length,resize?1:3);
  if(!resize)assert.deepEqual(h.sent.slice(1).map(x=>x.revision),['12','12']);
 }
});


test('new owner coalesced motion survives repaint behind a retired accepted flight',async()=>{
 let done;const first=new Promise(resolve=>done=resolve);let calls=0;
 const h=harness('any',()=>++calls===1?first:undefined);
 h.event('pointermove');h.setFrame({attachment:{},layout:8});
 h.event('pointermove',{clientX:70});h.setFrame({revision:12});
 h.event('pointermove',{clientX:70}); // repaint must retain dedup and pending motion
 assert.equal(h.sent.length,1);
 done();await settleWheel();
 assert.equal(h.sent.length,2);
 assert.deepEqual([h.sent[1].kind,h.sent[1].layout,h.sent[1].revision],['motion','8','12']);
 h.event('pointermove',{clientX:70});await settleWheel();assert.equal(h.sent.length,2);
});
