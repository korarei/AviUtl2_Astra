use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Borrow;
use std::collections::HashSet;
use std::fmt;
use std::hash::Hash;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Eq => "==",
            Self::Ne => "~=",
            Self::Gt => ">",
            Self::Lt => "<",
        })
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    var: &'a str,
    cond: Option<Cond<'a>>,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: &'a str,
        body: Option<&'a str>,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "hide" {
            return Ok(None);
        }

        if body.is_some_and(|body| body.chars().any(char::is_control)) {
            bail!("{pos}: property 'hide' cannot contain control characters");
        }

        let (cond, rest) = match body {
            None => (None, None),
            Some(body) => {
                if body.trim().is_empty() {
                    bail!("{pos}: property 'hide' requires a condition after ':'");
                }

                let (cond, rest) = body
                    .split_once(',')
                    .map_or((body.trim(), None), |(cond, rest)| (cond.trim(), Some(rest)));

                if let Some(rest) = rest {
                    tracing::warn!(
                        "{pos}: property 'hide' expects at most 1 argument, got {}",
                        rest.split(',').count() + 1
                    );
                }

                (Some(Cond::parse(&pos, cond)?), rest)
            }
        };

        Ok(Some(Self { pos, var, cond, rest }))
    }

    pub(in crate::build::exedit) fn validate<S>(&self, vars: &HashSet<S>) -> anyhow::Result<()>
    where
        S: Borrow<str> + Eq + Hash,
    {
        let pos = &self.pos;

        if !lua::is_ident(self.var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
            bail!(
                "{pos}: target variable '{}' of property 'hide' must be a Lua identifier",
                self.var
            );
        }

        if !vars.contains(self.var) {
            bail!("{pos}: undefined target variable '{}' of property 'hide'", self.var);
        }

        if let Some(cond) = self.cond {
            if !lua::is_ident(cond.var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                bail!(
                    "{pos}: condition variable '{}' of property 'hide' must be a Lua identifier",
                    cond.var
                );
            }

            if cond.var == self.var {
                bail!(
                    "{pos}: condition variable '{}' of property 'hide' cannot be the target variable",
                    cond.var
                );
            }

            if !matches!(cond.var, "filter" | "input" | "output") && !vars.contains(cond.var) {
                bail!("{pos}: undefined condition variable '{}' of property 'hide'", cond.var);
            }
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--hide@{}", self.var)?;

        if let Some(cond) = self.cond {
            write!(f, ":{cond}")?;

            if let Some(rest) = self.rest {
                write!(f, ",{rest}")?;
            }
        }

        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cond<'a> {
    var: &'a str,
    op: Op,
    val: i64,
}

#[allow(dead_code)]
impl<'a> Cond<'a> {
    fn parse(pos: &Pos, src: &'a str) -> anyhow::Result<Self> {
        let Some((at, raw, op)) = [("==", Op::Eq), ("~=", Op::Ne), (">", Op::Gt), ("<", Op::Lt)]
            .into_iter()
            .filter_map(|(raw, op)| src.find(raw).map(|at| (at, raw, op)))
            .min_by_key(|&(at, _, _)| at)
        else {
            bail!("{pos}: invalid condition '{src}' of property 'hide'");
        };

        let var = src[..at].trim();

        if var.is_empty() {
            bail!("{pos}: invalid condition '{src}' of property 'hide'");
        }

        let val = src[at + raw.len()..].trim();
        let Ok(val) = val.parse::<i64>() else {
            bail!("{pos}: value in condition of property 'hide' must be an integer, got '{val}'");
        };

        Ok(Self { var, op, val })
    }
}

impl fmt::Display for Cond<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}{}", self.var, self.op, self.val)
    }
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
    fn parses_hide_prop() {
        let unconditional = Prop::parse(pos(), "hide", "x", None).unwrap().unwrap();
        assert_eq!(unconditional.var, "x");
        assert_eq!(unconditional.to_string(), "--hide@x");

        for (body, expected) in [
            ("flag==0", "--hide@x:flag==0"),
            ("  mode ~= 1  ", "--hide@x:mode~=1"),
            ("sec > -1", "--hide@x:sec>-1"),
            ("input<3", "--hide@x:input<3"),
        ] {
            let prop = Prop::parse(pos(), "hide", "x", Some(body)).unwrap().unwrap();
            assert_eq!(prop.to_string(), expected);
        }

        let extra = Prop::parse(pos(), "hide", "x", Some("flag==0, extra, args"))
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--hide@x:flag==0, extra, args");

        assert!(Prop::parse(pos(), "file", "x", None).unwrap().is_none());

        for body in [
            Some(""),
            Some("   "),
            Some("flag==0\n"),
            Some("flag"),
            Some("flag=0"),
            Some("flag>=0"),
            Some("flag<=0"),
            Some("flag!=0"),
            Some("==0"),
            Some("flag=="),
            Some("flag==1.5"),
            Some("flag==0==1"),
        ] {
            assert!(Prop::parse(pos(), "hide", "x", body).is_err(), "{body:?}");
        }
    }

    #[test]
    fn validates_hide_prop() {
        let vars = HashSet::from(["x", "grp", "sep", "flag", "mode"]);

        for (var, body) in [
            ("x", None),
            ("grp", None),
            ("sep", Some("flag==0")),
            ("x", Some("mode~=2")),
            ("flag", Some("x==0")),
            ("x", Some("grp==0")),
            ("x", Some("filter==1")),
            ("x", Some("input>0")),
            ("x", Some("output<3")),
        ] {
            let prop = Prop::parse(pos(), "hide", var, body).unwrap().unwrap();
            prop.validate(&vars).unwrap();
        }

        let vars = HashSet::from(["x".to_owned(), "flag".to_owned()]);
        let owned = Prop::parse(pos(), "hide", "x", Some("flag==1")).unwrap().unwrap();
        owned.validate(&vars).unwrap();

        for (var, body) in [
            ("", None),
            ("1bad", None),
            ("local", None),
            ("missing", None),
            ("x", Some("1bad==0")),
            ("x", Some("a b==0")),
            ("x", Some("x==0")),
            ("x", Some("missing==0")),
        ] {
            let prop = Prop::parse(pos(), "hide", var, body).unwrap().unwrap();
            let err = prop.validate(&vars).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{var}:{body:?}: {err}");
        }
    }

    #[test]
    fn rejects_self_conditions() {
        let vars = HashSet::from(["x", "filter", "input", "output"]);

        for var in &vars {
            for op in ["==", "~=", ">", "<"] {
                let body = format!("{var}{op}0");
                let prop = Prop::parse(pos(), "hide", var, Some(&body)).unwrap().unwrap();
                let error = prop.validate(&vars).unwrap_err();

                assert!(
                    error.to_string().contains("cannot be the target variable"),
                    "{body}: {error}"
                );
            }
        }
    }
}
