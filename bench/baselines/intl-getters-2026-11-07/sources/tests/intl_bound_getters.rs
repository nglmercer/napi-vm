use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both_tiers(source: &str) {
    for prepared in [false, true] {
        let mut vm = Interpreter::with_builtins();
        let value = if prepared {
            vm.execute(&Interpreter::compile(source).unwrap())
        } else {
            let body = Parser::new(Lexer::new(source).tokenize())
                .parse_program()
                .unwrap();
            vm.run_program_body(&body)
        };
        assert!(
            matches!(value, Ok(Value::Bool(true))),
            "prepared={prepared}: {value:?}"
        );
    }
}

#[test]
fn cached_bound_methods_do_not_bypass_receiver_brand_checks() {
    both_tiers(
        r#"
      var n=new Intl.NumberFormat('en-US'),d=new Intl.DateTimeFormat('en-US',{timeZone:'UTC'}),c=new Intl.Collator('en-US');
      var nf=n.format,df=d.format,cf=c.compare;
      var ng=Object.getOwnPropertyDescriptor(Intl.NumberFormat.prototype,'format').get;
      var dg=Object.getOwnPropertyDescriptor(Intl.DateTimeFormat.prototype,'format').get;
      var cg=Object.getOwnPropertyDescriptor(Intl.Collator.prototype,'compare').get;

      var nw=Object.create(Intl.NumberFormat.prototype),dw=Object.create(Intl.DateTimeFormat.prototype);
      Intl.NumberFormat.call(nw,'en-US');Intl.DateTimeFormat.call(dw,'en-US');
      var ns=Object.getOwnPropertySymbols(nw),ds=Object.getOwnPropertySymbols(dw);
      var forgedN=Object.create(Intl.NumberFormat.prototype),forgedD=Object.create(Intl.DateTimeFormat.prototype);
      forgedN[ns[0]]=d;forgedD[ds[0]]=n;
      var cases=[[ng,forgedN],[dg,forgedD],[ng,d],[dg,n],[ng,c],[dg,c],[cg,n],[cg,d],[ng,{}],[dg,{}],[cg,{}]];
      var ok=ns.length===1&&ds.length===1&&nf===ng.call(n)&&df===dg.call(d)&&cf===cg.call(c);
      for(var pair of cases){try{pair[0].call(pair[1]);ok=false;}catch(e){ok=ok&&e instanceof TypeError;}}
      ok;
    "#,
    );
}

#[test]
fn proxy_receivers_preserve_legacy_unwrapping_and_direct_brand_checks() {
    both_tiers(
        r#"
      var types=[[Intl.NumberFormat,'format'],[Intl.DateTimeFormat,'format'],[Intl.Collator,'compare']];
      var ok=true;
      for(var pair of types){var traps=0,marker={},instance=new pair[0]('en-US');instance[pair[1]];
        var proxy=new Proxy(instance,{get(){traps++;throw marker;}});
        var getter=Object.getOwnPropertyDescriptor(pair[0].prototype,pair[1]).get;
        try{getter.call(proxy);ok=false;}catch(e){ok=ok&&(pair[1]==='compare'?traps===0&&e instanceof TypeError:traps===1&&e===marker);}
      }
      ok;
    "#,
    );
}

#[test]
fn legacy_wrappers_keep_per_instance_method_identity_and_descriptors() {
    both_tiers(
        r#"
      var a=Object.create(Intl.NumberFormat.prototype),b=Object.create(Intl.DateTimeFormat.prototype);
      Intl.NumberFormat.call(a,'en-US');Intl.DateTimeFormat.call(b,'en-US',{timeZone:'UTC',year:'numeric',month:'2-digit',day:'2-digit'});
      var c=new Intl.Collator('en-US'),other=new Intl.NumberFormat('en-US');
      var ok=a.format===a.format&&b.format===b.format&&c.compare===c.compare&&a.format!==other.format;
      var methods=[a.format,b.format,c.compare];
      for(var f of methods){var name=Object.getOwnPropertyDescriptor(f,'name');var len=Object.getOwnPropertyDescriptor(f,'length');
        ok=ok&&name.value===''&&!name.enumerable&&name.configurable&&!name.writable&&!len.enumerable&&len.configurable&&!len.writable&&Object.getOwnPropertyNames(f).join()==='length,name';}
      ok&&a.format(1234)==='1,234'&&b.format(1704067200000)==='01/01/2024'&&c.compare('a','b')<0;
    "#,
    );
}

#[test]
fn all_extracted_bound_methods_keep_receivers_alive_through_collection() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(r#"
      var n=new Intl.NumberFormat('en-US'),d=new Intl.DateTimeFormat('en-US',{timeZone:'UTC',year:'numeric',month:'2-digit',day:'2-digit'}),c=new Intl.Collator('en-US');
      var nf=n.format,df=d.format,cf=c.compare;n=null;d=null;c=null;
    "#).unwrap();
    vm.collect_cycles();
    let result=vm.eval_source("nf.call({},1234)==='1,234'&&df.call(null,1704067200000)==='01/01/2024'&&cf.call(undefined,'a','b')<0;");
    assert!(matches!(result, Ok(Value::Bool(true))), "{result:?}");
}
