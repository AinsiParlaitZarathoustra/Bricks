use bricks_agentlang::ast::*;
use bricks_agentlang::parse;

fn chain(e: &Expr) -> &Chain {
    match e {
        Expr::Chain(c) => c,
        other => panic!("expected chain, got {other:?}"),
    }
}

#[test]
fn parses_namespaced_call_with_var_arg() {
    let prog = parse("io.read($filepath)").unwrap();
    assert_eq!(prog.stmts.len(), 1);
    let Stmt::Expr(e) = &prog.stmts[0] else {
        panic!("expected expr stmt")
    };
    let c = chain(e);
    assert_eq!(c.head.path, vec!["io", "read"]);
    assert!(c.tail.is_empty());
    assert_eq!(c.head.args.len(), 1);
    match &c.head.args[0] {
        Arg::Positional(Expr::Var { name, .. }) => assert_eq!(name, "filepath"),
        other => panic!("expected $filepath, got {other:?}"),
    }
}

#[test]
fn parses_deep_namespace() {
    let prog = parse("agent.tools.call('Read')").unwrap();
    let Stmt::Expr(e) = &prog.stmts[0] else {
        panic!()
    };
    assert_eq!(chain(e).head.path, vec!["agent", "tools", "call"]);
}

#[test]
fn parses_method_chain() {
    let prog = parse("io.read($a).write($b).delete()").unwrap();
    let Stmt::Expr(e) = &prog.stmts[0] else {
        panic!()
    };
    let c = chain(e);
    assert_eq!(c.head.path, vec!["io", "read"]);
    assert_eq!(c.tail.len(), 2);
    assert_eq!(c.tail[0].path, vec!["write"]);
    assert_eq!(c.tail[1].path, vec!["delete"]);
}

#[test]
fn parses_named_args_and_literals() {
    let prog = parse(r#"io.write($f, content: "hi", perms: [1, 2, true])"#).unwrap();
    let Stmt::Expr(e) = &prog.stmts[0] else {
        panic!()
    };
    let c = chain(e);
    assert_eq!(c.head.args.len(), 3);
    match &c.head.args[1] {
        Arg::Named { name, value, .. } => {
            assert_eq!(name, "content");
            assert_eq!(*value, Expr::Literal(Literal::Str("hi".into())));
        }
        other => panic!("expected named arg, got {other:?}"),
    }
    match &c.head.args[2] {
        Arg::Named { name, value, .. } => {
            assert_eq!(name, "perms");
            assert!(matches!(value, Expr::Literal(Literal::Array(items)) if items.len() == 3));
        }
        other => panic!("expected named arg, got {other:?}"),
    }
}

#[test]
fn parses_assignment_and_comments() {
    let src = "# leading comment\n$x = net.get($url)  # trailing\n";
    let prog = parse(src).unwrap();
    assert_eq!(prog.stmts.len(), 1);
    match &prog.stmts[0] {
        Stmt::Assign { name, value, .. } => {
            assert_eq!(name, "x");
            assert_eq!(chain(value).head.path, vec!["net", "get"]);
        }
        other => panic!("expected assignment, got {other:?}"),
    }
}

#[test]
fn parses_multiple_statements() {
    let prog = parse("$a = io.read($f)\nio.write($g, content: $a)\n").unwrap();
    assert_eq!(prog.stmts.len(), 2);
}

#[test]
fn error_on_unterminated_string() {
    let err = parse("io.read('oops)").unwrap_err();
    assert!(err.message.contains("unterminated"), "{}", err.message);
}

#[test]
fn error_on_missing_paren() {
    let err = parse("io.read($f").unwrap_err();
    assert_eq!(err.span.line, 1);
}
