const {test} = require('node:test');
const assert = require('node:assert/strict');
const {spawnSync} = require('node:child_process');
const {threadId} = require('node:worker_threads');
const {AsyncSession, Vm} = require('../../index.js');
const runtimeAvailable = (() => {const vm = new Vm();try {vm.enableRuntime({timers:true});return true;}catch{return false;}finally{vm.dispose();}})();

test('persistent owner keeps globals across repeated executions', async () => {
  const s = new AsyncSession();
  try {
    assert.equal(await s.run('var count=0;'), 'undefined');
    for(let i=1;i<=100;i++) assert.equal(await s.run('++count;'), String(i));
    assert.equal(await s.getGlobal('count'), '100');
    await s.setGlobal('data',{items:[1,2,3]});
    assert.equal(await s.run('data.items.join();'),'1,2,3');
    await s.defineModule('numbers','export const n=42;');
    assert.equal(await s.run("import {n} from 'numbers';n;"),'42');
  } finally {s.dispose();}
});
test('host functions and Node marshalling execute on Node thread without sync VM deadlock', async () => {
  const s = new AsyncSession(); const sync=new Vm();
  try {
    await s.exposeFunction('host',n=>{assert.equal(threadId,0);return {answer:Number(sync.run(`${n}+1;`))};});
    assert.equal(await s.run('host(41).answer;'),'42');
    await s.exposeFunction('delayed',async n=>{await new Promise(r=>setTimeout(r,5));assert.equal(threadId,0);return n+2;},true);
    assert.equal(await s.run('await delayed(40);'),'42');
  } finally {s.dispose();sync.dispose();}
});
test('manual virtual turns preserve unfinished checkpoints and absolute timers', {skip: !runtimeAvailable},async()=>{
  const s=new AsyncSession({timers:true,clock:'virtual',autoPoll:false});
  try {
    await s.evaluate("var seen=[];setTimeout(()=>{seen.push('a');queueMicrotask(()=>seen.push('m'));setTimeout(()=>seen.push('nested'),5);},10);setTimeout(()=>seen.push('b'),10);");
    const idle=await s.pollEventLoop(10);assert.equal(idle.executedJobs,0);assert.equal(idle.nextDeadline,10);
    await s.advanceClock(10);
    const first=await s.pollEventLoop(1);assert.equal(first.checkpointPending,true);
    await assert.rejects(s.evaluate("seen.push('intruder');"),/checkpoint/);
    await s.pollEventLoop(1);await s.pollEventLoop(1);
    assert.equal(await s.getGlobal('seen'),'[a, m, b]');
    await s.advanceClock(5);await s.pollEventLoop(1);assert.equal(await s.getGlobal('seen'),'[a, m, b, nested]');
  } finally {s.dispose();}
});
test('bounded command queue rejects visibly without losing accepted completion',async()=>{
  const s=new AsyncSession({commandCapacity:1});
  try {const accepted=s.run('1+1;');assert.throws(()=>s.run('2+2;'),/queue is full/);assert.equal(await accepted,'2');assert.equal(await s.run('2+2;'),'4');}
  finally{s.dispose();}
});
test('same-session commands from a host function fail instead of deadlocking',async()=>{
  const s=new AsyncSession();
  try {
    await s.exposeFunction('reenter',()=>{assert.throws(()=>s.run('1;'),/awaiting Node/);return 42;});
    assert.equal(await s.run('reenter();'),'42');
    await s.exposeFunction('later',async()=>{await new Promise(r=>setTimeout(r,2));assert.throws(()=>s.getGlobal('x'),/awaiting Node/);return 7;},true);
    assert.equal(await s.run('await later();'),'7');
  }finally{s.dispose();}
});
test('deadline interrupts long callback and a never-settled host promise',async()=>{
  for(const host of [false,true]){
    const s=new AsyncSession();
    try {
      if(host) await s.exposeFunction('never',()=>new Promise(()=>{}),true);
      await s.setExecutionLimits(1000000000,10);
      await assert.rejects(s.run(host?'await never();':'while(true){}'),/deadline/);
    }finally{s.dispose();}
  }
});
test('shutdown cancels guest work and settles every pending completion',async()=>{
  const s=new AsyncSession({commandCapacity:4});
  const p=s.run('while(true){}');const q=s.run('42;');
  const observed=Promise.allSettled([p,q]);setTimeout(()=>{s.dispose();s.dispose();},5);
  const results=await observed;assert.equal(results[0].status,'rejected');assert.equal(results[1].status,'rejected');
  assert.throws(()=>s.run('1;'),/disposed/);
});
test('shutdown wakes an owner awaiting a never-settled Node completion',async()=>{
  const s=new AsyncSession();await s.exposeFunction('never',()=>new Promise(()=>{}),true);
  const p=s.run('await never();');setTimeout(()=>s.dispose(),5);await assert.rejects(p,/disposed|cancelled/);
});
test('idle sessions and pending completions have correct Node lifetimes', {skip: !runtimeAvailable},()=>{
  for(const source of [
    "new AsyncSession();",
    "const s=new AsyncSession({timers:true});s.run('40+2;').then(v=>{console.log(v);s.dispose()});",
    "const s=new AsyncSession({timers:true});s.evaluate('var values=[];for(var i=0;i<20;i++)setTimeout(()=>values.push(i),5)').then(()=>setTimeout(()=>s.dispose(),20));",
  ]) {
    const child=spawnSync(process.execPath,['-e',`const {AsyncSession}=require('./index.js');${source}`],{cwd:require('node:path').resolve(__dirname,'../..'),encoding:'utf8',timeout:2000});
    assert.equal(child.error,undefined,`${child.error} ${child.stderr}`);assert.equal(child.status,0,child.stderr);
    if(source.includes('40+2'))assert.equal(child.stdout.trim(),'42');
  }
});
test('immediate cancellation reaches commands not yet dequeued by owner',async()=>{
  const s=new AsyncSession({autoPoll:false});
  try {const p=s.run('while(true){}');s.cancel();await assert.rejects(p,/cancelled/);}
  finally{s.dispose();}
});
test('pending Node promise stays rooted until deadline and retires on disposal',()=>{
  const child=spawnSync(process.execPath,['--expose-gc','-e',`
    const {AsyncSession}=require('./index.js');
    (async()=>{const s=new AsyncSession();try{
      await s.exposeFunction('never',()=>new Promise(()=>{}),true);
      await s.setExecutionLimits(1000000000,40);
      const p=s.run('await never();');setTimeout(()=>global.gc(),5);
      await p;throw Error('unexpected resolution');
    }catch(e){if(!/deadline/.test(e.message))throw e;console.log('deadline');}finally{s.dispose();}})().catch(e=>{console.error(e);process.exitCode=1;});
  `],{cwd:require('node:path').resolve(__dirname,'../..'),encoding:'utf8',timeout:2000});
  assert.equal(child.error,undefined,String(child.error));assert.equal(child.status,0,child.stderr);assert.equal(child.stdout.trim(),'deadline');
});
test('Node worker teardown wakes and terminates the persistent guest owner',async()=>{
  const {Worker}=require('node:worker_threads');
  const path=require('node:path').resolve(__dirname,'../../index.js');
  const worker=new Worker(`const {parentPort}=require('node:worker_threads');const {AsyncSession}=require(${JSON.stringify(path)});const s=new AsyncSession();s.run('while(true){}').catch(()=>{});parentPort.postMessage('started');`,{eval:true});
  await new Promise((resolve,reject)=>{worker.once('message',resolve);worker.once('error',reject);});
  // Node and Bun differ in terminate()'s return value; both emit the actual
  // exit status. Verify the lifecycle event instead of the wrapper's result.
  const exited=new Promise((resolve,reject)=>{worker.once('exit',resolve);worker.once('error',reject);});
  let timer;
  worker.terminate();
  try {assert.equal(await Promise.race([exited,new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('worker teardown timed out')),2000);})]),1);}
  finally {clearTimeout(timer);await worker.terminate();}
});

test('throwing checkpoint recovers before new guest code', {skip: !runtimeAvailable}, () => {
  const vm = new Vm();
  vm.enableRuntime({timers:true});
  try {
    assert.throws(() => vm.run("queueMicrotask(() => { throw new Error('boom'); });"), /boom/);
    assert.equal(vm.run('42;'), '42');
    assert.throws(() => vm.run("var seen=[];queueMicrotask(()=>{throw new Error('boom')});queueMicrotask(()=>seen.push('queued'));"), /boom/);
    assert.equal(vm.run("seen.push('new');seen.join(',');"), 'queued,new');
  } finally { vm.dispose(); }
});
test('unawaited host results do not lock admission and remain awaitable', async () => {
  const s = new AsyncSession();
  let resolve;
  let releaseDispatch;
  const dispatch = new Promise(r => { releaseDispatch = r; });
  let notifyStarted;
  const started = new Promise(r => { notifyStarted = r; });
  try {
    await s.exposeFunction('background', async () => 1, true);
    assert.equal(await s.run('background();42;'), '42');
    assert.equal(await s.run('2+2;'), '4');
    await s.exposeFunction('pending', async () => {
      // Force the resolver to become available after both guest commands.
      // A run's completion does not imply its queued host callback has run.
      await dispatch;
      return new Promise(r => { resolve = r; notifyStarted(); });
    }, true);
    assert.equal(await s.run('var saved=pending();42;'), '42');
    assert.equal(await s.run('2+2;'), '4');
    releaseDispatch();
    await started;
    resolve(7);
    assert.equal(await s.run('await saved;'), '7');
    await s.exposeFunction('bad', async () => { throw new Error('rejected'); }, true);
    await s.run('var rejected=bad();');
    await assert.rejects(s.run('await rejected;'), /rejected/);
  } finally { releaseDispatch(); s.dispose(); }
});
test('top-level await waits for future and nested real-time timers', {skip: !runtimeAvailable}, async () => {
  const s = new AsyncSession({timers:true, clock: 'real-time' });
  try {
    assert.equal(await s.run('await new Promise(r=>setTimeout(()=>r(42),50));'), '42');
    assert.equal(await s.run('await new Promise(r=>setTimeout(()=>setTimeout(()=>r(7),5),5));'), '7');
    await assert.rejects(s.run("await new Promise((r,j)=>setTimeout(()=>j(new Error('timer rejection')),5));"), /timer rejection/);
    await assert.rejects(s.run('await new Promise(()=>{});'), /no VM or host event/);
  } finally { s.dispose(); }
});
test('cancel and dispose wake a future-timer await after a host barrier', {skip: !runtimeAvailable}, async () => {
  for (const dispose of [false,true]) {
    const s = new AsyncSession({timers:true, clock: 'real-time' });
    let started;
    const barrier = new Promise(r => { started = r; });
    await s.exposeFunction('started', () => { started(); return 0; });
    const p = s.run('started();await new Promise(r=>setTimeout(()=>r(42),60000));');
    await barrier;
    if (dispose) s.dispose(); else s.cancel();
    await assert.rejects(p, /cancelled|disposed/);
    s.dispose();
  }
});

test('abandoned results are retired while closure-held results survive', async () => {
  const s = new AsyncSession();
  try {
    await s.exposeFunction('background', async () => 7, true);
    for (let i=0; i<1050; i++) assert.equal(await s.run('background();42;'), '42');
    await s.run('var read;{let saved=background();read=()=>saved;}');
    assert.equal(await s.run('await read();'), '7');
  } finally { s.dispose(); }
});
test('virtual top-level await never advances time and supports host-driven recovery', {skip: !runtimeAvailable}, async () => {
  const s = new AsyncSession({timers:true,clock:'virtual', autoPoll:false});
  try {
    await s.evaluate('var saved=new Promise(r=>setTimeout(()=>r(42),50));');
    await assert.rejects(s.run('await saved;'), /host-driven timer progress/);
    assert.equal((await s.pollEventLoop(10)).nextDeadline, 50);
    await s.advanceClock(50);
    await s.pollEventLoop(10);
    assert.equal(await s.run('await saved;'), '42');
  } finally { s.dispose(); }
});
test('a short execution deadline caps a far-future timer await', {skip: !runtimeAvailable, timeout:2000}, async () => {
  const s = new AsyncSession({timers:true,clock:'real-time'});
  try {
    await s.setExecutionLimits(1000000000, 5);
    await assert.rejects(s.run('await new Promise(r=>setTimeout(()=>r(42),60000));'), /deadline/);
  } finally { s.dispose(); }
});

test('dispatched host dependency rejects reentry before guest await', {timeout:3000}, async () => {
  for (const delayed of [false, true]) {
    const s = new AsyncSession(); const other = new AsyncSession();
    let started, release, attempted, observed;
    const atCallback = new Promise(r => { started = r; });
    const continuation = new Promise(r => { release = r; });
    const atAttempt = new Promise(r => { attempted = r; });
    try {
      await s.setExecutionLimits(10000000000);
      await s.exposeFunction('background', async () => {
        started();
        if (delayed) await continuation;
        try { await s.run('2+2;'); observed = 'accepted'; }
        catch (error) { observed = error.message; }
        finally { attempted(); }
        return 1;
      }, true);
      // The guest stays in pure execution until cancelled, so no await_host
      // guard or synchronous host barrier can mask the dispatch window.
      const work = s.run('var saved=background();while(true){};await saved;');
      const rejectedWork = assert.rejects(work, /cancelled/);
      await atCallback;
      assert.equal(await other.run('42;'), '42');
      release();
      // A queued reentry must not hang the test: cancellation releases the old
      // implementation's accepted command; its error then fails this assertion.
      await new Promise(r => setImmediate(r));
      s.cancel();
      await atAttempt; await rejectedWork;
      assert.match(observed, /awaiting Node|host.call dependency/);
    } finally { s.dispose(); other.dispose(); }
  }
});

test('completed deadlines do not poison idle metadata or new executions', async () => {
  const s = new AsyncSession();
  try {
    await s.setExecutionLimits(1000000000, 50);
    await s.run('var answer=42;');
    await new Promise(r => setTimeout(r,100));
    assert.equal(await s.getGlobal('answer'),'42');
    assert.equal(await s.run('2+2;'),'4');
  } finally { s.dispose(); }
});

test('delayed unawaited callback can reenter after its execution completes', async () => {
  const s = new AsyncSession();
  let release, completed;
  const callbackComplete = new Promise(resolve => { completed = resolve; });
  const continuation = new Promise(resolve => { release = resolve; });
  try {
    await s.exposeFunction('background', async () => {
      await continuation;
      assert.equal(await s.run('2+2;'), '4');
      completed();
      return 7;
    }, true);
    assert.equal(await s.run('var saved=background();42;'), '42');
    assert.equal(await s.run('3+3;'), '6');
    release();
    // Wait for callback reentry before asking the owner to await its result.
    await callbackComplete;
    assert.equal(await s.run('await saved;'), '7');
  } finally { s.dispose(); }
});

test('metadata and configured limits preserve pending timer deadlines', {skip: !runtimeAvailable}, async () => {
  const s = new AsyncSession({timers:true,clock:'real-time', autoPoll:false});
  try {
    await s.setExecutionLimits(1_000_000_000, 50);
    await s.evaluate('var answer=42;setTimeout(()=>answer++,1000000);');
    await s.setExecutionLimits(1_000_000_000, 5000);
    assert.equal(await s.getGlobal('answer'), '42');
    await new Promise(resolve => setTimeout(resolve, 100));
    await assert.rejects(s.pollEventLoop(10), /deadline/);
  } finally { s.dispose(); }
});

test('a saved host result protects reentry across executions and queued admission', {timeout:5000}, async () => {
  const s = new AsyncSession();
  const other = new AsyncSession();
  let release, observed;
  const continuation = new Promise(resolve => { release = resolve; });
  try {
    await s.setExecutionLimits(10_000_000_000, 2000);
    await s.exposeFunction('background', async () => {
      await continuation;
      try { await s.run('2+2;'); observed = 'accepted'; }
      catch (error) { observed = error.message; }
      return 7;
    }, true);
    assert.equal(await s.run('var saved=background();42;'), '42');
    // Admit B before releasing A's callback. Pure guest work precedes await;
    // neither a new host dispatch nor a synchronous Node wait masks the gap.
    const b = s.run('for(var i=0;i<100000;i++){};await saved;');
    release();
    assert.equal(await b, '7');
    assert.match(observed, /awaiting Node|host.call dependency/);
    assert.equal(await other.run('42;'), '42');
    assert.equal(await s.run('2+2;'), '4');
  } finally { s.dispose(); other.dispose(); }
});
