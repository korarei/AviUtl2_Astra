use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Runtime {
    LuaJit,
    Lua,
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LuaJit => "luaJIT",
            Self::Lua => "lua",
        })
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Desc<'a> {
    pos: Pos,
    runtime: Runtime,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Desc<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "script" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: descriptor 'script' cannot contain control characters");
        }

        if body.trim().is_empty() {
            bail!("{pos}: descriptor 'script' requires 1 argument, got 0");
        }

        let (runtime, rest) = body
            .split_once(',')
            .map_or((body.trim(), None), |(runtime, rest)| (runtime.trim(), Some(rest)));

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: descriptor 'script' expects 1 argument, got {}",
                rest.split(',').count() + 1
            );
        }

        let runtime = match runtime {
            "luaJIT" => Runtime::LuaJit,
            "lua" => Runtime::Lua,
            _ => bail!("{pos}: runtime of descriptor 'script' must be 'luaJIT' or 'lua', got '{runtime}'"),
        };

        Ok(Some(Self { pos, runtime, rest }))
    }

    #[expect(clippy::unnecessary_wraps)]
    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        if self.runtime == Runtime::Lua {
            tracing::warn!("{}: runtime of descriptor 'script' is 'lua'", self.pos);
        }

        Ok(())
    }
}

impl fmt::Display for Desc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--script:{}", self.runtime)?;

        if let Some(rest) = self.rest {
            write!(f, ",{rest}")?;
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
            file: Path::new("input.anm2").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_script_desc() {
        for (body, expected) in [
            ("luaJIT", "--script:luaJIT"),
            ("  luaJIT  ", "--script:luaJIT"),
            ("lua", "--script:lua"),
            ("  lua  ", "--script:lua"),
            ("luaJIT, extra, args", "--script:luaJIT, extra, args"),
        ] {
            let desc = Desc::parse(pos(), "script", body).unwrap().unwrap();
            assert_eq!(desc.to_string(), expected);
            desc.validate().unwrap();
        }

        assert!(Desc::parse(pos(), "require", "luaJIT").unwrap().is_none());

        for body in ["", "   ", ", luaJIT", "LuaJIT", "luajit", "Lua", "Python", "luaJIT\n"] {
            let err = Desc::parse(pos(), "script", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{body}: {err}");
        }
    }
}
