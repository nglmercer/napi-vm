//! Compile-only companion to audit_syntax.py; never executes guest JavaScript.
use napi_vm_core::{parser::ParseGoal, Interpreter};
use std::io::{self, BufRead};

fn main() {
    for line in io::stdin().lock().lines() {
        let input: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let goal = if input["module"].as_bool().unwrap() {
            ParseGoal::Module
        } else {
            ParseGoal::Script
        };
        let result = Interpreter::compile_with_goal(input["source"].as_str().unwrap(), goal);
        println!(
            "{}",
            serde_json::json!({
                "test": input["test"], "variant": input["variant"],
                "accepted": result.is_ok(), "error": result.err().map(|error| error.to_string())
            })
        );
    }
}
