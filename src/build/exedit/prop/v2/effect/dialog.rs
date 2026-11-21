use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    Nil,
    Int(i64),
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => f.write_str("nil"),
            Self::Int(val) => write!(f, "0x{val:06x}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Value<'a> {
    Plain(&'a str),
    Check(bool),
    Color(Color),
    Figure(&'a str),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    items: Vec<Item<'a>>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: Option<&'a str>,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "dialog" {
            return Ok(None);
        }

        if var.is_some() {
            bail!("{pos}: property 'dialog' cannot have a variable");
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'dialog' cannot contain control characters");
        }

        let body = body.trim();
        let body = body.strip_suffix(';').unwrap_or(body);
        if body.is_empty() {
            bail!("{pos}: property 'dialog' requires at least 1 item");
        }

        let mut items = Vec::new();
        for item in body.split(';') {
            let Some((name, assign)) = item.split_once(',') else {
                bail!("{pos}: item of 'dialog' must contain ',', got '{item}'");
            };

            let Some((var, val)) = assign.split_once('=') else {
                bail!("{pos}: item of 'dialog' must contain '=', got '{item}'");
            };

            let name = name.trim();
            let var = var.trim();
            let val = val.trim();

            let (name, val) = if let Some(name) = name.strip_suffix("/chk") {
                let is_checked = match val {
                    "0" => false,
                    "1" => true,
                    _ => bail!("{pos}: value of '/chk' in 'dialog' must be '0' or '1', got '{val}'"),
                };
                (name, Value::Check(is_checked))
            } else if let Some(name) = name.strip_suffix("/col") {
                let Some(color) = parse_color(val) else {
                    bail!(
                        "{pos}: value of '/col' in 'dialog' must be 'nil' or an integer between 0x000000 and 0xffffff, got '{val}'"
                    );
                };
                (name, Value::Color(color))
            } else if let Some(name) = name.strip_suffix("/fig") {
                (name, Value::Figure(val))
            } else {
                (name, Value::Plain(val))
            };

            items.push(Item {
                var,
                name,
                key: Cow::Borrowed(name),
                val,
            });
        }

        let mut counts = HashMap::new();

        for item in items.iter_mut().rev() {
            let count = counts.entry(item.name).or_insert(0);

            if *count > 0 {
                item.key = Cow::Owned(format!("{}{}", "dialog::".repeat(*count), item.name));
            }

            *count += 1;
        }

        Ok(Some(Self { pos, items }))
    }

    pub(in crate::build::exedit) fn names(&self) -> Vec<&str> {
        self.items.iter().map(|item| item.key.as_ref()).collect()
    }

    pub(in crate::build::exedit) fn labels(&self) -> Vec<&str> {
        self.items
            .iter()
            .map(|item| item.name.rsplit_once("::").map_or(item.name, |(_, label)| label))
            .collect()
    }

    pub(in crate::build::exedit) fn validate(
        &self,
        vars: &mut HashSet<&'a str>,
        names: &mut HashSet<Cow<'a, str>>,
    ) -> anyhow::Result<()> {
        let pos = &self.pos;

        tracing::warn!("{pos}: property 'dialog' uses legacy syntax");

        for item in &self.items {
            let var = item.var;
            let name = item.name;

            if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                bail!("{pos}: variable name '{var}' of property 'dialog' must be a Lua identifier");
            }

            if !vars.insert(var) {
                bail!("{pos}: duplicate variable name '{var}'");
            }

            if name.starts_with("effect.") {
                bail!("{pos}: property name must not start with 'effect.', got '{name}'");
            }

            if lua::parse_number(name).is_some() {
                bail!("{pos}: property name must not be a number, got '{name}'");
            }

            if !names.insert(item.key.clone()) {
                bail!("{pos}: duplicate property name '{}'", item.key);
            }
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("--dialog:")?;

        for item in &self.items {
            write!(f, "{item};")?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item<'a> {
    var: &'a str,
    name: &'a str,
    key: Cow<'a, str>,
    val: Value<'a>,
}

impl fmt::Display for Item<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.val {
            Value::Plain(val) => write!(f, "{},{}={val}", self.name, self.var),
            Value::Check(val) => write!(f, "{}/chk,{}={}", self.name, self.var, u8::from(val)),
            Value::Color(val) => write!(f, "{}/col,{}={val}", self.name, self.var),
            Value::Figure(val) => write!(f, "{}/fig,{}={val}", self.name, self.var),
        }
    }
}

fn parse_color(src: &str) -> Option<Color> {
    if src == "nil" {
        return Some(Color::Nil);
    }

    lua::parse_integer(src)
        .filter(|&val| (0..=0xff_ffff).contains(&val))
        .map(Color::Int)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pos() -> Pos {
        Pos {
            file: Path::new("input.anm2").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_dialog_prop() {
        let prop = Prop::parse(
            pos(),
            "dialog",
            None,
            " Xオフセット , x = 100 ; 有効/chk , flag = 1 ; Group::色/col , col = 255 ; 空色/col , empty = nil ; 図形/fig , fig = \"四角形\" ; ",
        )
        .unwrap()
        .unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        prop.validate(&mut vars, &mut names).unwrap();
        assert_eq!(
            prop.to_string(),
            "--dialog:Xオフセット,x=100;有効/chk,flag=1;Group::色/col,col=0x0000ff;空色/col,empty=nil;図形/fig,fig=\"四角形\";"
        );

        assert_eq!(prop.names(), ["Xオフセット", "有効", "Group::色", "空色", "図形"]);
        assert_eq!(prop.labels(), ["Xオフセット", "有効", "色", "空色", "図形"]);

        let dup_names = Prop::parse(pos(), "dialog", None, "A,a=1;B,b=2;A,c=3;A,d=4")
            .unwrap()
            .unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        dup_names.validate(&mut vars, &mut names).unwrap();
        assert_eq!(dup_names.names(), ["dialog::dialog::A", "B", "dialog::A", "A"]);
        assert_eq!(dup_names.labels(), ["A", "B", "A", "A"]);
        assert_eq!(dup_names.to_string(), "--dialog:A,a=1;B,b=2;A,c=3;A,d=4;");

        let without_trailing = Prop::parse(pos(), "dialog", None, "A,a=0").unwrap().unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        without_trailing.validate(&mut vars, &mut names).unwrap();
        assert_eq!(without_trailing.to_string(), "--dialog:A,a=0;");

        assert!(Prop::parse(pos(), "param", None, "a=0").unwrap().is_none());
    }

    #[test]
    fn preserves_scoped_duplicate_names() {
        let prop = Prop::parse(
            pos(),
            "dialog",
            None,
            "Group::日本,a=1;Group::日本/chk,b=0;Group::日本/fig,c=\"x\"",
        )
        .unwrap()
        .unwrap();
        prop.validate(&mut HashSet::new(), &mut HashSet::new()).unwrap();
        assert_eq!(
            prop.names(),
            ["dialog::dialog::Group::日本", "dialog::Group::日本", "Group::日本"]
        );
        assert_eq!(prop.labels(), ["日本", "日本", "日本"]);
        assert_eq!(
            prop.to_string(),
            "--dialog:Group::日本,a=1;Group::日本/chk,b=0;Group::日本/fig,c=\"x\";"
        );
    }

    #[test]
    fn allows_many_items_without_v0_limits() {
        let body = (0..20)
            .map(|i| format!("有効{i}/chk,c{i}=0;色{i}/col,col{i}=nil;図形{i}/fig,f{i}=\"円\""))
            .collect::<Vec<_>>()
            .join(";");
        let prop = Prop::parse(pos(), "dialog", None, &body).unwrap().unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        prop.validate(&mut vars, &mut names).unwrap();
        assert_eq!(prop.names().len(), 60);
    }

    #[test]
    fn rejects_invalid_dialog_prop() {
        assert!(Prop::parse(pos(), "dialog", Some("x"), "A,a=1").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "A,a=1\0").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "  ").is_err());
        assert!(Prop::parse(pos(), "dialog", None, ";").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "A").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "A,a").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "有効/chk,flag=2").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "有効/chk,flag=true").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "色/col,col=0x1000000").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "色/col,col=-1").is_err());
        assert!(Prop::parse(pos(), "dialog", None, "色/col,col=red").is_err());

        for body in [
            "A,1x=1",
            "A,end=1",
            "effect.foo,x=1",
            "123,x=1",
            "A,x=1;B,x=2",
            "dialog::A,a=1;A,b=2;A,c=3",
        ] {
            let prop = Prop::parse(pos(), "dialog", None, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            assert!(prop.validate(&mut vars, &mut names).is_err(), "{body}");
        }

        let prop = Prop::parse(pos(), "dialog", None, "A,x=1").unwrap().unwrap();
        let mut vars = HashSet::from(["x"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("A")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
