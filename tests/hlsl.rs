use astra::build::hlsl::{self, Lexer, State, TokenKind};

#[test]
fn distinguishes_hlsl_directives_from_comments_and_literals() {
    let src = "//#define VALUE 1\n/* #if FLAG */\n\"#pragma pack_matrix(row_major)\"\nfloat x; #endif\n #pragma pack_matrix(column_major)\n";
    let tokens = Lexer::new(src).collect::<Vec<_>>();
    assert_eq!(
        tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Directive)
            .map(|token| token.text)
            .collect::<Vec<_>>(),
        ["#pragma pack_matrix(column_major)"]
    );
    assert!(tokens.iter().any(|token| {
        matches!(token.kind, TokenKind::Comment { is_block: false }) && token.text == "//#define VALUE 1"
    }));
    assert!(
        tokens
            .iter()
            .any(|token| { token.kind == TokenKind::String && token.text == "\"#pragma pack_matrix(row_major)\"" })
    );
    assert!(tokens.iter().any(|token| token.kind == TokenKind::Other('#')));
}

#[test]
fn preserves_comments_and_directive_continuations_across_chunks() {
    let mut state = State::default();
    let mut tokens = Vec::new();
    for src in [
        "#define MACRO \\",
        "cbuffer Hidden { int x; }; \\",
        "// continued",
        "/*",
        "#pragma pack_matrix(row_major)",
        "*/ cbuffer Visible { float4 x; };",
    ] {
        let mut lexer = Lexer::new(src);
        lexer.state = state;
        tokens.extend(lexer.by_ref());
        state = lexer.state;
    }
    assert_eq!(
        tokens.iter().filter(|token| token.kind == TokenKind::Directive).count(),
        1
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Continuation)
            .count(),
        2
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Ident && token.text == "cbuffer")
            .count(),
        1
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Ident && token.text == "Visible")
    );
    assert!(!state.is_comment);
    assert!(!state.is_directive);
}

#[test]
fn ends_directive_continuation_on_an_empty_chunk() {
    let mut state = State::default();
    let mut tokens = Vec::new();
    for src in ["#define MACRO \\", "", "cbuffer Visible { float4 x; };"] {
        let mut lexer = Lexer::new(src);
        lexer.state = state;
        tokens.extend(lexer.by_ref());
        state = lexer.state;
    }
    assert!(!tokens.iter().any(|token| token.kind == TokenKind::Continuation));
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Ident && token.text == "cbuffer")
    );
}

#[test]
fn preserves_source_spans_for_unicode_and_escaped_strings() {
    let src = "cbuffer 色 { float4 値; }; \"日本語\\\"/* #if */\" // コメント\n0xff 1.5e-2 .5f";
    let tokens = Lexer::new(src).collect::<Vec<_>>();
    for token in &tokens {
        assert!(src.is_char_boundary(token.span.st));
        assert!(src.is_char_boundary(token.span.ed));
        assert_eq!(token.text, &src[token.span.st..token.span.ed]);
    }
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Ident && token.text == "値")
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Number)
            .map(|token| token.text)
            .collect::<Vec<_>>(),
        ["0xff", "1.5e-2", ".5f"]
    );
    assert!(!tokens.iter().any(|token| token.kind == TokenKind::Directive));
}

#[test]
fn parses_types_and_declarations_with_source_spans() {
    let src = "row_major matrix<float, 2, 3> m[2], n";
    let tokens = Lexer::new(src)
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. }
            )
        })
        .collect::<Vec<_>>();
    let declaration = hlsl::parse_declaration(&tokens).unwrap();
    assert_eq!(
        declaration.ty,
        Some(hlsl::Type {
            scalar: "float",
            rows: 2,
            cols: 3,
            is_matrix: true
        })
    );
    assert_eq!(declaration.is_row_major, Some(true));
    assert!(!declaration.has_error);
    assert_eq!(declaration.members.len(), 2);
    assert_eq!(declaration.members[0].name, "m");
    assert_eq!(declaration.members[0].dimensions, [Some(2)]);
    assert_eq!(
        &src[declaration.members[0].span.st..declaration.members[0].span.ed],
        "m"
    );
    assert_eq!(declaration.members[1].name, "n");
    assert!(declaration.members[1].dimensions.is_empty());
}

#[test]
fn preserves_user_types_and_unknown_array_lengths() {
    let tokens = Lexer::new("Custom value[COUNT]")
        .filter(|token| token.kind != TokenKind::Whitespace)
        .collect::<Vec<_>>();
    let declaration = hlsl::parse_declaration(&tokens).unwrap();
    assert_eq!(declaration.ty.unwrap().scalar, "Custom");
    assert!(!declaration.has_error);
    assert_eq!(declaration.members[0].name, "value");
    assert_eq!(declaration.members[0].dimensions, [None]);
}

#[test]
fn preserves_valid_members_before_an_incomplete_declaration() {
    let tokens = Lexer::new("float good, broken[2")
        .filter(|token| token.kind != TokenKind::Whitespace)
        .collect::<Vec<_>>();
    let declaration = hlsl::parse_declaration(&tokens).unwrap();
    assert!(declaration.has_error);
    assert_eq!(declaration.members.len(), 1);
    assert_eq!(declaration.members[0].name, "good");
}

#[test]
fn parses_native_directives_without_astra_directives() {
    let src = "# pragma pack_matrix(row_major)";
    let directive = hlsl::parse_directive(Lexer::new(src).next().unwrap()).unwrap();
    assert_eq!(directive.name, "pragma");
    assert_eq!(directive.rest, "pack_matrix(row_major)");
    assert_eq!(&src[directive.span.st..directive.span.ed], src);
    assert!(hlsl::parse_directive(Lexer::new("//#define VALUE 1").next().unwrap()).is_none());
}
