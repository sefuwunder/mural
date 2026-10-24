//! Vim-style `:` command-line parser for mural.
//!
//! SDL-free: `parse` turns a typed line into a [`Command`]; main.rs owns the
//! input bar, history display, and execution against App state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolName {
    Pencil,
    Pen,
    Marker,
    OilStick,
    Charcoal,
    Smudge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prop {
    Size,
    Spacing,
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOp {
    Get,
    Set,
    Add,
    Sub,
}

/// A parsed property value. Size/spacing are floats, color is an index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SetVal {
    F(f32),
    U(usize),
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Write(Option<String>),
    Quit { force: bool },
    Edit(String),
    Tool(ToolName),
    Set {
        prop: Prop,
        op: SetOp,
        value: Option<SetVal>,
    },
    Color(usize),
    Export(String),
    Undo,
    Redo,
    Help,
    Mixer,
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    Empty,
    UnknownCommand(String),
    UnknownProperty(String),
    UnknownTool(String),
    AmbiguousTool(String, Vec<String>),
    BadValue(String),
    MissingArg(&'static str),
    UnexpectedBang,
}

impl ParseError {
    /// One-line message for the command-line status bar.
    pub fn message(&self) -> String {
        match self {
            ParseError::Empty => String::new(),
            ParseError::UnknownCommand(w) => format!("E: unknown command '{w}' (try :help)"),
            ParseError::UnknownProperty(p) => format!("E: unknown property '{p}'"),
            ParseError::UnknownTool(t) => format!("E: unknown tool '{t}'"),
            ParseError::AmbiguousTool(t, cands) => {
                format!("E: ambiguous tool '{t}': {}", cands.join(", "))
            }
            ParseError::BadValue(v) => format!("E: bad value '{v}'"),
            ParseError::MissingArg(what) => format!("E: missing {what}"),
            ParseError::UnexpectedBang => "E: '!' only works with :q".to_string(),
        }
    }
}

const TOOL_NAMES: [(&str, ToolName); 6] = [
    ("pencil", ToolName::Pencil),
    ("pen", ToolName::Pen),
    ("marker", ToolName::Marker),
    ("oil", ToolName::OilStick),
    ("charcoal", ToolName::Charcoal),
    ("smudge", ToolName::Smudge),
];

/// Resolve a tool name by exact match, else unique prefix (`:tool char`).
pub fn resolve_tool(s: &str) -> Result<ToolName, ParseError> {
    let s = s.to_lowercase();
    if let Some((_, t)) = TOOL_NAMES.iter().find(|(n, _)| *n == s) {
        return Ok(*t);
    }
    let cands: Vec<&(&str, ToolName)> =
        TOOL_NAMES.iter().filter(|(n, _)| n.starts_with(s.as_str())).collect();
    match cands.as_slice() {
        [] => Err(ParseError::UnknownTool(s)),
        [(_, t)] => Ok(*t),
        _ => Err(ParseError::AmbiguousTool(
            s,
            cands.iter().map(|(n, _)| n.to_string()).collect(),
        )),
    }
}

/// (command name, shortest accepted abbreviation length).
/// `:e` is edit even though it also prefixes `export` (vim convention).
const COMMANDS: [(&str, usize); 11] = [
    ("write", 1),
    ("quit", 1),
    ("edit", 1),
    ("tool", 1),
    ("set", 2),
    ("color", 1),
    ("export", 2),
    ("undo", 1),
    ("redo", 1),
    ("help", 1),
    ("mixer", 1),
];

fn resolve_command(word: &str) -> Result<&'static str, ParseError> {
    if let Some((name, _)) = COMMANDS.iter().find(|(n, _)| *n == word) {
        return Ok(name);
    }
    let cands: Vec<&&str> = COMMANDS
        .iter()
        .filter(|(n, min)| n.starts_with(word) && word.len() >= *min)
        .map(|(n, _)| n)
        .collect();
    match cands.as_slice() {
        [] => Err(ParseError::UnknownCommand(word.to_string())),
        [name] => Ok(name),
        _ => Err(ParseError::UnknownCommand(word.to_string())),
    }
}

fn parse_set_val(prop: Prop, v: &str) -> Result<SetVal, ParseError> {
    match prop {
        Prop::Size | Prop::Spacing => v
            .parse::<f32>()
            .map(SetVal::F)
            .map_err(|_| ParseError::BadValue(v.to_string())),
        Prop::Color => v
            .parse::<usize>()
            .map(SetVal::U)
            .map_err(|_| ParseError::BadValue(v.to_string())),
    }
}

fn parse_set(args: &str) -> Result<Command, ParseError> {
    let args = args.trim();
    if args.is_empty() {
        return Err(ParseError::MissingArg("property"));
    }
    // operator forms: `size+=2`, `size-=2`, `size=24`
    let (prop_s, op, val_s): (&str, SetOp, Option<&str>) = if let Some(i) = args.find("+=") {
        (&args[..i], SetOp::Add, Some(args[i + 2..].trim()))
    } else if let Some(i) = args.find("-=") {
        (&args[..i], SetOp::Sub, Some(args[i + 2..].trim()))
    } else if let Some(i) = args.find('=') {
        (&args[..i], SetOp::Set, Some(args[i + 1..].trim()))
    } else {
        // space-separated `size 24`, or bare `size` (get)
        let mut parts = args.split_whitespace();
        let p = parts.next().unwrap_or("");
        match parts.next() {
            Some(v) => (p, SetOp::Set, Some(v)),
            None => (p, SetOp::Get, None),
        }
    };
    let prop_s = prop_s.trim();
    let prop = match prop_s {
        "size" => Prop::Size,
        "spacing" => Prop::Spacing,
        "color" => Prop::Color,
        _ => return Err(ParseError::UnknownProperty(prop_s.to_string())),
    };
    let value = match (op, val_s) {
        (SetOp::Get, _) => None,
        (_, Some(v)) if v.is_empty() => return Err(ParseError::BadValue("empty value".into())),
        (_, Some(v)) => Some(parse_set_val(prop, v)?),
        (_, None) => return Err(ParseError::MissingArg("value")),
    };
    Ok(Command::Set { prop, op, value })
}

/// Parse one command line (leading `:` optional) into a [`Command`].
pub fn parse(line: &str) -> Result<Command, ParseError> {
    let line = line.trim();
    let line = line.strip_prefix(':').unwrap_or(line).trim();
    if line.is_empty() {
        return Err(ParseError::Empty);
    }
    let (word, rest) = match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    };
    let (word, force) = match word.strip_suffix('!') {
        Some(w) if !w.is_empty() => (w, true),
        _ => (word, false),
    };
    let cmd = resolve_command(&word.to_lowercase())?;
    if force && cmd != "quit" {
        return Err(ParseError::UnexpectedBang);
    }
    match cmd {
        "write" => Ok(Command::Write(if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        })),
        "quit" => Ok(Command::Quit { force }),
        "edit" => {
            if rest.is_empty() {
                Err(ParseError::MissingArg("path"))
            } else {
                Ok(Command::Edit(rest.to_string()))
            }
        }
        "tool" => {
            if rest.is_empty() {
                Err(ParseError::MissingArg("tool name"))
            } else {
                let name = rest.split_whitespace().next().unwrap();
                Ok(Command::Tool(resolve_tool(name)?))
            }
        }
        "set" => parse_set(rest),
        "color" => match rest.split_whitespace().next() {
            Some(v) => v
                .parse::<usize>()
                .map(Command::Color)
                .map_err(|_| ParseError::BadValue(v.to_string())),
            None => Err(ParseError::MissingArg("color index")),
        },
        "export" => {
            if rest.is_empty() {
                Err(ParseError::MissingArg("path"))
            } else {
                Ok(Command::Export(rest.to_string()))
            }
        }
        "undo" => Ok(Command::Undo),
        "redo" => Ok(Command::Redo),
        "help" => Ok(Command::Help),
        "mixer" => Ok(Command::Mixer),
        _ => Err(ParseError::UnknownCommand(word.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviations() {
        assert_eq!(parse("w"), Ok(Command::Write(None)));
        assert_eq!(parse("write"), Ok(Command::Write(None)));
        assert_eq!(parse("w /tmp/a.json"), Ok(Command::Write(Some("/tmp/a.json".into()))));
        assert_eq!(parse("q"), Ok(Command::Quit { force: false }));
        assert_eq!(parse("q!"), Ok(Command::Quit { force: true }));
        assert_eq!(parse("quit!"), Ok(Command::Quit { force: true }));
        assert_eq!(parse("e /tmp/a.json"), Ok(Command::Edit("/tmp/a.json".into())));
        assert_eq!(parse("se"), Err(ParseError::MissingArg("property")));
        assert_eq!(parse("col 3"), Ok(Command::Color(3)));
        assert_eq!(parse("exp /tmp/a.png"), Ok(Command::Export("/tmp/a.png".into())));
        assert_eq!(parse("u"), Ok(Command::Undo));
        assert_eq!(parse("r"), Ok(Command::Redo));
        assert_eq!(parse("h"), Ok(Command::Help));
        // leading colon is tolerated
        assert_eq!(parse(":w"), Ok(Command::Write(None)));
    }

    #[test]
    fn set_forms() {
        assert_eq!(
            parse("set size"),
            Ok(Command::Set { prop: Prop::Size, op: SetOp::Get, value: None })
        );
        assert_eq!(
            parse("set size=24"),
            Ok(Command::Set { prop: Prop::Size, op: SetOp::Set, value: Some(SetVal::F(24.0)) })
        );
        assert_eq!(
            parse("set size 24"),
            Ok(Command::Set { prop: Prop::Size, op: SetOp::Set, value: Some(SetVal::F(24.0)) })
        );
        assert_eq!(
            parse("set size+=2"),
            Ok(Command::Set { prop: Prop::Size, op: SetOp::Add, value: Some(SetVal::F(2.0)) })
        );
        assert_eq!(
            parse("set size -= 2"),
            Ok(Command::Set { prop: Prop::Size, op: SetOp::Sub, value: Some(SetVal::F(2.0)) })
        );
        assert_eq!(
            parse("set spacing=0.2"),
            Ok(Command::Set { prop: Prop::Spacing, op: SetOp::Set, value: Some(SetVal::F(0.2)) })
        );
        assert_eq!(
            parse("set color=3"),
            Ok(Command::Set { prop: Prop::Color, op: SetOp::Set, value: Some(SetVal::U(3)) })
        );
    }

    #[test]
    fn set_errors() {
        assert_eq!(parse("set siez=3"), Err(ParseError::UnknownProperty("siez".into())));
        assert_eq!(parse("set size=abc"), Err(ParseError::BadValue("abc".into())));
        assert_eq!(parse("set size="), Err(ParseError::BadValue("empty value".into())));
        assert_eq!(parse("set"), Err(ParseError::MissingArg("property")));
        assert_eq!(parse("set color=x"), Err(ParseError::BadValue("x".into())));
    }

    #[test]
    fn unknown_command() {
        assert_eq!(parse("frobnicate"), Err(ParseError::UnknownCommand("frobnicate".into())));
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert_eq!(parse("w!"), Err(ParseError::UnexpectedBang));
    }

    #[test]
    fn tool_prefix_matching() {
        assert_eq!(parse("tool char"), Ok(Command::Tool(ToolName::Charcoal)));
        assert_eq!(parse("tool smudge"), Ok(Command::Tool(ToolName::Smudge)));
        assert_eq!(parse("tool o"), Ok(Command::Tool(ToolName::OilStick)));
        // "pen" is an exact name and wins over the "pencil" prefix
        assert_eq!(parse("tool pen"), Ok(Command::Tool(ToolName::Pen)));
        assert_eq!(parse("tool penc"), Ok(Command::Tool(ToolName::Pencil)));
        assert_eq!(
            parse("tool p"),
            Err(ParseError::AmbiguousTool("p".into(), vec!["pencil".into(), "pen".into()]))
        );
        assert_eq!(parse("tool crayon"), Err(ParseError::UnknownTool("crayon".into())));
        assert_eq!(parse("tool"), Err(ParseError::MissingArg("tool name")));
    }

    #[test]
    fn error_messages() {
        assert_eq!(
            ParseError::UnknownProperty("siez".into()).message(),
            "E: unknown property 'siez'"
        );
        assert!(ParseError::UnknownCommand("x".into()).message().starts_with("E: "));
    }
}
