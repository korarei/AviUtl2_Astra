use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Value<'a> {
    Plain(&'a str),
    Check(bool),
    Color(i64),
    Figure(&'a str),
}

impl fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(val) | Self::Figure(val) => f.write_str(val),
            Self::Check(val) => write!(f, "{}", u8::from(*val)),
            Self::Color(val) => write!(f, "{val}"),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    items: Vec<Item<'a>>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "dialog" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'dialog' cannot contain control characters");
        }

        let src = body.trim();
        let src = src.strip_suffix(';').map_or(src, str::trim_end);

        if src.is_empty() {
            bail!("{pos}: property 'dialog' requires at least 1 item, got 0");
        }

        let items = src
            .split(';')
            .map(|item| {
                let Some((name, assign)) = item.split_once(',') else {
                    bail!("{pos}: item of property 'dialog' must contain ',', got '{item}'");
                };

                let Some((var, val)) = assign.split_once('=') else {
                    bail!("{pos}: item of property 'dialog' must contain '=', got '{item}'");
                };

                let name = name.trim();
                let var = var.trim();
                let val = val.trim();

                if var.is_empty() {
                    bail!("{pos}: variable name of item in property 'dialog' cannot be empty");
                }

                if val.is_empty() {
                    bail!("{pos}: default value of '{var}' in property 'dialog' cannot be empty");
                }

                let val = match name.rsplit_once('/').map(|(_, suffix)| suffix) {
                    Some("chk") => match val {
                        "1" => Value::Check(true),
                        "0" => Value::Check(false),
                        _ => bail!("{pos}: value of '/chk' in property 'dialog' must be '0' or '1', got '{val}'"),
                    },
                    Some("col") => {
                        let Some(color) = lua::parse_integer(val) else {
                            bail!("{pos}: value of '/col' in property 'dialog' must be an integer, got '{val}'");
                        };

                        Value::Color(color)
                    }
                    Some("fig") => Value::Figure(val),
                    _ => Value::Plain(val),
                };

                Ok(Item { name, var, val })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Some(Self { pos, items }))
    }

    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.items.len() > 16 {
            bail!(
                "{pos}: property 'dialog' accepts at most 16 items, got {}",
                self.items.len()
            );
        }

        let mut counts = [0usize; 3];

        for (i, item) in self.items.iter().enumerate() {
            if !lua::is_ident(item.var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                bail!(
                    "{pos}: variable name '{}' of property 'dialog' must be a Lua identifier",
                    item.var
                );
            }

            if self.items[..i].iter().any(|prev| prev.var == item.var) {
                bail!("{pos}: duplicate variable name '{}' in property 'dialog'", item.var);
            }

            match item.val {
                Value::Plain(_) => {}
                Value::Check(_) => counts[0] += 1,
                Value::Color(val) => {
                    if !(0..=0xff_ffff).contains(&val) {
                        bail!(
                            "{pos}: value of '/col' in property 'dialog' must be between 0x000000 and 0xffffff, got {val}"
                        );
                    }

                    counts[1] += 1;
                }
                Value::Figure(_) => counts[2] += 1,
            }
        }

        for (suffix, count) in [("chk", counts[0]), ("col", counts[1]), ("fig", counts[2])] {
            if count > 4 {
                bail!("{pos}: property 'dialog' accepts at most 4 '/{suffix}' items, got {count}");
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

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct Item<'a> {
    name: &'a str,
    var: &'a str,
    val: Value<'a>,
}

impl fmt::Display for Item<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}={}", self.name, self.var, self.val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pos() -> Pos {
        Pos {
            file: Path::new("input.anm").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_dialog_prop() {
        let prop = Prop::parse(pos(), "dialog", " Xオフセット , x = 100 ; Yオフセット , y = 100 ; ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.to_string(), "--dialog:Xオフセット,x=100;Yオフセット,y=100;");

        let typed = Prop::parse(
            pos(),
            "dialog",
            r#"サイズ,size=100;有効/chk,on=1;色/col,col=0xffffff;図形/fig,fig="四角形""#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            typed.to_string(),
            r#"--dialog:サイズ,size=100;有効/chk,on=1;色/col,col=16777215;図形/fig,fig="四角形";"#
        );

        assert!(Prop::parse(pos(), "param", "x=100").unwrap().is_none());

        for body in [
            "",
            "   ",
            ";",
            "X",
            "X,x",
            "X,=100",
            "X,x=",
            "X,x=1;Y,y=2;;",
            "Check/chk,b=true",
            "Check/chk,b=2",
            "Color/col,c=nil",
            "Color/col,c=abc",
            "X,x=100\n",
        ] {
            let err = Prop::parse(pos(), "dialog", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn validates_dialog_prop() {
        let valid = Prop::parse(
            pos(),
            "dialog",
            r#"A/chk,c1=0;B/chk,c2=1;C/chk,c3=0;D/chk,c4=1;E/col,o1=0;F/col,o2=0xffffff;G/col,o3=0x123456;H/col,o4=1;I/fig,f1="円";J/fig,f2="円";K/fig,f3="円";L/fig,f4="円";M,v1=1;N,v2=2;O,v3=3;P,v4=4"#,
        )
        .unwrap()
        .unwrap();
        valid.validate().unwrap();

        let seventeen = (0..17).map(|i| format!("N{i},v{i}=0")).collect::<Vec<_>>().join(";");
        let five_chk = (0..5).map(|i| format!("C{i}/chk,c{i}=0")).collect::<Vec<_>>().join(";");
        let five_col = (0..5).map(|i| format!("O{i}/col,o{i}=0")).collect::<Vec<_>>().join(";");
        let five_fig = (0..5)
            .map(|i| format!("F{i}/fig,f{i}=\"円\""))
            .collect::<Vec<_>>()
            .join(";");

        for body in [
            "X,local=1",
            "X,1a=1",
            "X,x=1;Y,x=2",
            "Color/col,c=-1",
            "Color/col,c=16777216",
            &seventeen,
            &five_chk,
            &five_col,
            &five_fig,
        ] {
            let err = Prop::parse(pos(), "dialog", body)
                .unwrap()
                .unwrap()
                .validate()
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }
}
