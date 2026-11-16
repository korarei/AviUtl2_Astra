use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
const STEPS: &[f64] = &[1.0, 0.1, 0.01];

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    key: &'a str,
    name: &'a str,
    min: f64,
    max: f64,
    default: f64,
    step: Option<f64>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &'a str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if !matches!(kind, "track0" | "track1" | "track2" | "track3") {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property '{kind}' cannot contain control characters");
        }

        let mut parts = body.split(',');
        let name = parts.next().unwrap().trim();
        let count = parts.clone().count();

        if count < 3 {
            bail!("{pos}: property '{kind}' requires at least 3 arguments, got {count}");
        }

        if count > 4 {
            bail!("{pos}: property '{kind}' accepts at most 4 arguments, got {count}");
        }

        let parse = |field: &str, arg: &str| -> anyhow::Result<f64> {
            let arg = arg.trim();
            let Some(val) = lua::parse_number(arg).filter(|val| val.is_finite()) else {
                bail!("{pos}: {field} of property '{kind}' must be a number, got '{arg}'");
            };

            Ok(val)
        };

        let min = parse("minimum value", parts.next().unwrap())?;
        let max = parse("maximum value", parts.next().unwrap())?;
        let default = parse("default value", parts.next().unwrap())?;
        let step = parts
            .next()
            .map(|arg| {
                let val = parse("step size", arg)?;
                if !STEPS.iter().any(|&v| (val - v).abs() < f64::EPSILON) {
                    bail!("{pos}: step size of property '{kind}' must be 1, 0.1, or 0.01, got '{val}'");
                }

                Ok(val)
            })
            .transpose()?;

        Ok(Some(Self {
            pos,
            key: kind,
            name,
            min,
            max,
            default,
            step,
        }))
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn validate(&self, keys: &mut HashSet<&'a str>) -> anyhow::Result<()> {
        let pos = &self.pos;
        let key = self.key;
        let min = self.min;
        let max = self.max;
        let default = self.default;

        if !keys.insert(key) {
            bail!("{pos}: duplicate property key '{key}'");
        }

        if min > max {
            bail!("{pos}: minimum value ({min}) of property '{key}' must not exceed maximum value ({max})");
        }

        if !(min <= default && default <= max) {
            bail!("{pos}: default value of property '{key}' must be between {min} and {max}, got {default}");
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "--{}:{},{},{},{}",
            self.key, self.name, self.min, self.max, self.default
        )?;

        if let Some(step) = self.step {
            write!(f, ",{step}")?;
        }

        Ok(())
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
    fn parses_track_prop() {
        let prop = Prop::parse(pos(), "track0", "  Label  , 0 , 100 , 50 , 0.01 ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.key, "track0");
        assert_eq!(prop.name(), "Label");
        assert_eq!(prop.to_string(), "--track0:Label,0,100,50,0.01");

        let without_step = Prop::parse(pos(), "track3", "Scale,-1,1,0").unwrap().unwrap();
        assert_eq!(without_step.to_string(), "--track3:Scale,-1,1,0");

        let hex = Prop::parse(pos(), "track0", "Label,+0x2,0xff,0x1.fp3,0x1")
            .unwrap()
            .unwrap();
        assert_eq!(hex.to_string(), "--track0:Label,2,255,15.5,1");

        let bin = Prop::parse(pos(), "track1", "Mask,-0b10,+0b100,0b1").unwrap().unwrap();
        assert_eq!(bin.to_string(), "--track1:Mask,-2,4,1");

        for (kind, body) in [
            ("track4", "Label,0,100,0"),
            ("track@value", "Label,0,100,0"),
            ("check0", "Enable,0"),
        ] {
            assert!(Prop::parse(pos(), kind, body).unwrap().is_none(), "{kind}: {body}");
        }

        for body in [
            "Value",
            "Value,",
            "Value,0,1",
            "Value,NaN,1,0",
            "Value,0,inf,0",
            "Value,0,1,_",
            "Value,0,1,NaN",
            "Value,0,1,0,0",
            "Value,0,1,0,0.001",
            "Value,0,1,0,NaN",
            "Value,0,1,0,1,extra",
        ] {
            let err = Prop::parse(pos(), "track0", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn validates_track_prop() {
        for (kind, body) in [("track0", "Value,0,1,0,0.01"), ("track2", "Value,0,1,0")] {
            let prop = Prop::parse(pos(), kind, body).unwrap().unwrap();
            assert!(prop.validate(&mut HashSet::new()).is_ok(), "{kind}: {body}");
        }

        for body in ["Value,0,1,2", "Value,2,1,1"] {
            let err = Prop::parse(pos(), "track0", body)
                .unwrap()
                .unwrap()
                .validate(&mut HashSet::new())
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }

        let mut keys = HashSet::new();
        for kind in ["track0", "track1", "track2", "track3"] {
            Prop::parse(pos(), kind, "Value,0,1,0")
                .unwrap()
                .unwrap()
                .validate(&mut keys)
                .unwrap();
        }
        assert_eq!(keys, HashSet::from(["track0", "track1", "track2", "track3"]));

        let duplicate = Prop::parse(pos(), "track0", "Other,0,1,0").unwrap().unwrap();
        let err = duplicate.validate(&mut keys).unwrap_err();
        assert_eq!(err.to_string(), "input.anm:2:3: duplicate property key 'track0'");
    }
}
