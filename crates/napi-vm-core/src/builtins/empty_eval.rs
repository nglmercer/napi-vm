//! Recognize eval scripts consisting only of empty statements and blocks,
//! whitespace and comments, without allocating tokens or an AST. Everything
//! outside this restricted grammar is handled by the full parser.
use crate::{Interpreter, JsString, VmErr};

pub(super) fn statement_count(
    interp: &Interpreter,
    source: &JsString,
) -> Result<Option<u64>, VmErr> {
    let units = source.units();
    let (mut position, mut depth, mut statements, mut comment, mut next_poll) =
        (0, 0usize, 0u64, 0, 0);
    while position < units.len() {
        if position >= next_poll {
            interp.check_execution_interrupt()?;
            next_poll = position.saturating_add(4096);
        }
        let unit = units[position];
        if comment == 1 {
            if matches!(unit, 0x000A | 0x000D | 0x2028 | 0x2029) {
                comment = 0;
            }
        } else if comment == 2 {
            if unit == 0x002A && units.get(position + 1) == Some(&0x002F) {
                position += 1;
                comment = 0;
            }
        } else {
            match unit {
                0x0009..=0x000D
                | 0x0020
                | 0x00A0
                | 0x1680
                | 0x2000..=0x200A
                | 0x2028
                | 0x2029
                | 0x202F
                | 0x205F
                | 0x3000
                | 0xFEFF => {}
                0x003B => statements += 1,
                0x007B => {
                    depth += 1;
                    statements += 1;
                    // A conservative subset of the recursive parser's limit.
                    if depth > 32 {
                        return Ok(None);
                    }
                }
                0x007D => {
                    if depth == 0 {
                        return Ok(None);
                    }
                    depth -= 1;
                }
                0x002F if units.get(position + 1) == Some(&0x002F) => {
                    comment = 1;
                    position += 1;
                }
                0x002F if units.get(position + 1) == Some(&0x002A) => {
                    comment = 2;
                    position += 1;
                }
                _ => return Ok(None),
            }
        }
        position += 1;
    }
    interp.check_execution_interrupt()?;
    Ok((depth == 0 && comment != 2).then_some(statements))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lexer, Parser, Value};

    #[test]
    fn accepted_scripts_match_full_parser_completion_and_statement_fuel() {
        let mut interp = Interpreter::with_builtins();
        for source in [
            "",
            "{}",
            "{{};{;}};;;",
            "/*{;*/{ // }\n;} // tail",
            "\u{FEFF}{\u{2028};\u{2029}}",
        ] {
            let ast = Parser::new_with_spans(Lexer::new(source).tokenize_with_spans())
                .parse_program()
                .unwrap();
            let cost = statement_count(&interp, &source.into())
                .unwrap()
                .expect(source);
            let before = interp.execution_budget().fuel;
            assert!(matches!(
                interp.run_program_body(&ast).unwrap(),
                Value::Undefined
            ));
            assert_eq!(before - interp.execution_budget().fuel, cost, "{source}");
            if cost > 0 {
                interp.set_fuel_budget(cost - 1);
                assert!(interp.consume_fuel(cost).is_err());
                interp.set_fuel_budget(crate::interpreter::DEFAULT_FUEL_BUDGET);
            }
        }
    }

    #[test]
    fn malformed_nonempty_and_deep_sources_use_full_parser() {
        let interp = Interpreter::with_builtins();
        for source in [
            "{",
            "}",
            "{} }",
            "/*",
            "{ // }",
            "{} 1;",
            "'use strict';",
            "/x/",
            "/* */ /",
            "[{}]",
            "({})",
            "\u{0085}",
        ] {
            assert_eq!(
                statement_count(&interp, &source.into()).unwrap(),
                None,
                "{source}"
            );
        }
        let deep = "{".repeat(33) + &"}".repeat(33);
        assert_eq!(statement_count(&interp, &deep.into()).unwrap(), None);
        assert_eq!(
            statement_count(&interp, &JsString::from_units(vec![0xD800])).unwrap(),
            None
        );
    }

    #[test]
    fn scan_observes_cancellation() {
        let interp = Interpreter::with_builtins();
        interp.cancellation_token().cancel();
        assert!(statement_count(&interp, &"{}".repeat(100_000).into()).is_err());
    }
}
