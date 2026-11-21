use super::{desc, l10n, prop};
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub(in crate::build::exedit) struct Annotation<'a> {
    pub kind: &'a str,
    pub var: Option<&'a str>,
    pub body: Option<&'a str>,
}

impl<'a> Annotation<'a> {
    pub(in crate::build::exedit) fn parse(src: &'a str) -> Option<Self> {
        if src.is_empty() || src.starts_with(char::is_whitespace) {
            return None;
        }

        let (head, body) = match src.split_once(':') {
            Some((head, body)) => (head, Some(body)),
            None => (src, None),
        };
        let (kind, var) = match head.split_once('@') {
            Some((kind, var)) => (kind.trim_end(), Some(var.trim())),
            None => (head.trim_end(), None),
        };

        Some(Self { kind, var, body })
    }
}

pub(super) enum Element<'a> {
    Prop(prop::Prop<'a>),
    Desc(desc::v2::Desc<'a>),
}

impl<'a> Element<'a> {
    pub(super) fn can_bind(&self) -> bool {
        matches!(self, Self::Prop(prop) if prop.can_bind())
    }

    pub(super) fn process(
        self,
        binding: Option<Binding<'a, '_>>,
        state: &mut State<'a>,
        l10n: &mut l10n::Collector,
    ) -> anyhow::Result<String> {
        if binding.is_some() && !self.can_bind() {
            bail!("element cannot bind to a variable");
        }

        match self {
            Self::Prop(prop) => prop.process(binding, state, l10n),
            Self::Desc(desc) => desc.process(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::build::exedit) struct Binding<'a, 'b> {
    pub var: &'a str,
    pub val: Option<&'b str>,
}

#[derive(Default)]
pub(in crate::build::exedit) struct State<'a> {
    pub keys: HashSet<&'a str>,
    pub vars: HashSet<&'a str>,
    pub names: HashSet<Cow<'a, str>>,
}
