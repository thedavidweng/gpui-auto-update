//! Reads the selectors an Objective-C protocol declares from a framework
//! header, so the bundled tests check the Sparkle that is actually
//! embedded rather than a hand-copied list that can drift from it.

/// One protocol's selectors, split by `@required` and `@optional`.
#[derive(Debug, Default)]
pub struct ProtocolSelectors {
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

impl ProtocolSelectors {
    pub fn declares(&self, selector: &str) -> bool {
        self.required
            .iter()
            .chain(&self.optional)
            .any(|declared| declared == selector)
    }
}

/// The instance selectors (methods and readonly or readwrite property
/// getters) that `@protocol name` declares in `header`, or `None` when the
/// header does not declare that protocol.
pub fn protocol_selectors(header: &str, name: &str) -> Option<ProtocolSelectors> {
    let source = strip_parentheses(&strip_comments(header));
    let start = source
        .match_indices("@protocol")
        .map(|(index, _)| index)
        .find(|&index| {
            let rest = source[index + "@protocol".len()..].trim_start();
            rest.strip_prefix(name)
                .is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
                // `@protocol Name;` is a forward declaration.
                && !rest[name.len()..].trim_start().starts_with(';')
        })?;
    let body = &source[start..];
    let end = body.find("@end")?;
    let mut selectors = ProtocolSelectors::default();
    let mut required = true;
    for statement in body[..end].split(';') {
        // A statement may begin with the protocol's own header or with
        // section markers before its declaration.
        let tokens: Vec<&str> = statement.split_whitespace().collect();
        for (index, token) in tokens.iter().enumerate() {
            match *token {
                "@required" => required = true,
                "@optional" => required = false,
                "-" | "@property" => {
                    if let Some(selector) = selector_of(&tokens[index..]) {
                        if required {
                            selectors.required.push(selector);
                        } else {
                            selectors.optional.push(selector);
                        }
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    Some(selectors)
}

/// The selector of one method or property declaration, with types,
/// attributes, and macro arguments already removed.
fn selector_of(tokens: &[&str]) -> Option<String> {
    match *tokens.first()? {
        "-" => {
            let pieces: Vec<&str> = tokens[1..]
                .iter()
                .copied()
                .filter(|token| token.ends_with(':'))
                .collect();
            Some(if pieces.is_empty() {
                (*tokens.get(1)?).to_owned()
            } else {
                pieces.concat()
            })
        }
        "@property" => {
            // Attributes such as `NS_SWIFT_NAME` follow the name; the name
            // is the last token before them that is not a macro.
            let name = tokens[1..]
                .iter()
                .rev()
                .find(|token| !token.chars().all(|c| c.is_ascii_uppercase() || c == '_'))?;
            Some(name.trim_start_matches('*').to_owned())
        }
        _ => None,
    }
}

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |end| &after[end + 2..]);
            out.push(' ');
        } else if let Some(after) = rest.strip_prefix("//") {
            rest = after.find('\n').map_or("", |end| &after[end..]);
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Removes every parenthesized group (types, blocks, macro arguments),
/// nested ones included.
fn strip_parentheses(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut depth = 0_usize;
    for c in source.chars() {
        match c {
            '(' => {
                if depth == 0 {
                    out.push(' ');
                }
                depth += 1;
            }
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Checks the parser on a header shaped like Sparkle's.
pub fn self_test() {
    let header = r"
        @protocol Forward;
        /* A comment with - (void)notASelector:(int)x; inside */
        @protocol Example <NSObject>
        - (void)showThing:(Thing *)thing state:(State *)state reply:(void (^)(Choice))reply;
        // - (void)commentedOut;
        - (void)dismiss;
        @optional
        @property (nonatomic, readonly) BOOL supportsThing;
        - (BOOL)example:(id)e shouldWait:(void (^)(void))block NS_SWIFT_ASYNC(2);
        @end
        @protocol ExampleTwo
        - (void)other;
        @end
    ";
    let example = protocol_selectors(header, "Example").expect("Example is declared");
    assert_eq!(example.required, ["showThing:state:reply:", "dismiss"]);
    assert_eq!(example.optional, ["supportsThing", "example:shouldWait:"]);
    assert!(protocol_selectors(header, "Forward").is_none());
    assert_eq!(
        protocol_selectors(header, "ExampleTwo").unwrap().required,
        ["other"]
    );
}
