// Shared source cases cross the native and WASM execution boundaries.
module.exports = [
  {
    name: 'mixed UTF-16 property updates and method calls preserve key identity',
    source: String.raw`let keys=['plain','😀','\uD800','\uDC00','\uFFFD','\uFDD0sD800','#public'];
      if(keys[1].length!==2 || keys[1].charCodeAt(0)!==0xD83D || keys[1].charCodeAt(1)!==0xDE00 ||
         keys[2].length!==1 || keys[2].charCodeAt(0)!==0xD800 ||
         keys[3].length!==1 || keys[3].charCodeAt(0)!==0xDC00 ||
         keys[4].length!==1 || keys[4].charCodeAt(0)!==0xFFFD ||
         keys[5].length!==6 || keys[5].charCodeAt(0)!==0xFDD0)throw 'wrong fixture code units';
      let o={};
      for(let k of keys){o[k]=1;o[k]++;o[k]+=2;o[k]&&=5;}
      for(let k of keys){if(o[k]!==5)throw 'aliased update';}
      let methods={};
      for(let k of keys){methods[k]=function(){return this===methods;};}
      methods.plain() && methods['😀']() && methods['\uD800']() && methods['\uDC00']() &&
      methods['\uFFFD']() && methods['\uFDD0sD800']() && methods['#public']() &&
      Object.keys(o).length===keys.length && Object.keys(o).every((k,i)=>k===keys[i]);`,
    expected: 'true',
  },
  {
    name: 'warm property access survives data and accessor descriptor transitions',
    source: `var o={x:0,y:0};
      function read(){return o.x;}
      function write(v){o.x=v;return o.x;}
      for(var i=0;i<40;i++){write(i);read();}
      var log=[read()];var stored=50,hits=0;
      Object.defineProperty(o,'x',{get:function(){hits++;return stored;},set:function(v){hits++;stored=v;},configurable:true,enumerable:false});
      log.push(write(60));log.push(read());
      Object.defineProperty(o,'x',{value:70,writable:true,configurable:true,enumerable:true});
      log.push(write(80));delete o.x;o.x=90;
      log.push(read());log.push(Object.keys(o).join(','));log.push(hits);JSON.stringify(log);`,
    expected: '[39,60,60,80,90,"y,x",3]',
  },
  {
    name: 'own __proto__ data writes preserve the prototype and property attributes',
    source: `var p={marker:1};var o={};
      Object.defineProperty(o,'__proto__',{value:1,writable:true,enumerable:true,configurable:true});
      Object.setPrototypeOf(o,p);o.__proto__={marker:2};
      var accepted=Reflect.set(o,'__proto__',7);var d=Object.getOwnPropertyDescriptor(o,'__proto__');
      JSON.stringify([o.__proto__,Object.getPrototypeOf(o)===p,accepted,d.writable,d.enumerable,d.configurable,Object.keys(o).join(',')]);`,
    expected: '[7,true,true,true,true,true,"__proto__"]',
  },
];
