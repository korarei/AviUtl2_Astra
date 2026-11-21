use super::desc;
use super::element::{Annotation, Element};
use super::l10n;
use super::prop;
use crate::build::source::Pos;
use anyhow::bail;

pub(in crate::build::exedit) trait Spec {
    const HAS_SHADER: bool = false;
    const L10N: l10n::Format = l10n::Format::None;

    fn is_known_prop(annotation: &Annotation<'_>) -> bool;

    fn is_bindable(annotation: &Annotation<'_>) -> bool {
        let _ = annotation;
        false
    }

    fn reject_prop(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
        reject_v2_prop(suffix, annotation)
    }

    fn is_known_desc(annotation: &Annotation<'_>) -> bool {
        let _ = annotation;
        false
    }

    fn reject_desc(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
        reject_v2_desc(suffix, annotation)
    }

    fn parse_known(pos: Pos, annotation: Annotation<'_>) -> anyhow::Result<Option<Element<'_>>>;
}

pub(in crate::build::exedit) mod v2 {
    pub(in crate::build::exedit) struct Anm;
    pub(in crate::build::exedit) struct Effect;
    pub(in crate::build::exedit) struct Tra;
}

pub(in crate::build::exedit) mod v0 {
    pub(in crate::build::exedit) struct Effect;
    pub(in crate::build::exedit) struct Scn;
    pub(in crate::build::exedit) struct Tra;
}

pub(in crate::build::exedit) struct Unsupported;

impl Spec for v2::Anm {
    const HAS_SHADER: bool = true;
    const L10N: l10n::Format = l10n::Format::Full;

    fn is_known_prop(annotation: &Annotation<'_>) -> bool {
        v2::Effect::is_known_prop(annotation)
    }

    fn is_bindable(annotation: &Annotation<'_>) -> bool {
        v2::Effect::is_bindable(annotation)
    }

    fn reject_prop(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
        v2::Effect::reject_prop(suffix, annotation)
    }

    fn is_known_desc(annotation: &Annotation<'_>) -> bool {
        annotation.var.is_none()
            && (desc::v2::general::is_kind(annotation.kind, annotation.body.is_some())
                || desc::v2::anm::is_kind(annotation.kind))
    }

    fn reject_desc(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
        if annotation.var.is_none() && desc::v2::tra::is_kind(annotation.kind) {
            bail!("descriptor '{}' is not supported in '{suffix}'", annotation.kind);
        }

        Ok(())
    }

    fn parse_known(pos: Pos, annotation: Annotation<'_>) -> anyhow::Result<Option<Element<'_>>> {
        if let Some(element) = v2::Effect::parse_known(pos.clone(), annotation)? {
            return Ok(Some(element));
        }

        if annotation.var.is_none()
            && let Some(desc) = desc::v2::anm::parse(pos, annotation.kind, annotation.body.unwrap_or_default())?
        {
            return Ok(Some(Element::Desc(desc::v2::Desc::Filter(desc))));
        }

        Ok(None)
    }
}

impl Spec for v2::Effect {
    const HAS_SHADER: bool = true;
    const L10N: l10n::Format = l10n::Format::Full;

    fn is_known_prop(annotation: &Annotation<'_>) -> bool {
        prop::v2::effect::is_kind(annotation.kind, annotation.var.is_some(), annotation.body.is_some())
    }

    fn is_bindable(annotation: &Annotation<'_>) -> bool {
        prop::v2::effect::is_bindable(annotation.kind, annotation.var.is_some(), annotation.body.is_some())
    }

    fn reject_prop(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
        if annotation.var.is_none() && prop::v2::tra::is_kind(annotation.kind, annotation.body.is_some()) {
            bail!("property '{}' is not supported in '{suffix}'", annotation.kind);
        }

        Ok(())
    }

    fn is_known_desc(annotation: &Annotation<'_>) -> bool {
        annotation.var.is_none() && desc::v2::general::is_kind(annotation.kind, annotation.body.is_some())
    }

    fn reject_desc(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
        if annotation.var.is_none()
            && (desc::v2::anm::is_kind(annotation.kind) || desc::v2::tra::is_kind(annotation.kind))
        {
            bail!("descriptor '{}' is not supported in '{suffix}'", annotation.kind);
        }

        Ok(())
    }

    fn parse_known(pos: Pos, annotation: Annotation<'_>) -> anyhow::Result<Option<Element<'_>>> {
        if let Some(prop) =
            prop::v2::effect::Prop::parse(pos.clone(), annotation.kind, annotation.var, annotation.body)?
        {
            return Ok(Some(Element::Prop(prop::Prop::V2(prop::v2::Prop::Effect(prop)))));
        }

        if annotation.var.is_none()
            && let Some(desc) = desc::v2::general::Desc::parse(pos, annotation.kind, annotation.body)?
        {
            return Ok(Some(Element::Desc(desc::v2::Desc::General(desc))));
        }

        Ok(None)
    }
}

impl Spec for v2::Tra {
    const L10N: l10n::Format = l10n::Format::Basic;

    fn is_known_prop(annotation: &Annotation<'_>) -> bool {
        annotation.var.is_none() && prop::v2::tra::is_kind(annotation.kind, annotation.body.is_some())
    }

    fn reject_prop(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
        if prop::v2::effect::is_kind(annotation.kind, annotation.var.is_some(), annotation.body.is_some()) {
            bail!("property '{}' is not supported in '{suffix}'", annotation.kind);
        }

        Ok(())
    }

    fn is_known_desc(annotation: &Annotation<'_>) -> bool {
        annotation.var.is_none()
            && (desc::v2::general::is_kind(annotation.kind, annotation.body.is_some())
                || desc::v2::tra::is_kind(annotation.kind))
    }

    fn reject_desc(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
        if annotation.var.is_none() && desc::v2::anm::is_kind(annotation.kind) {
            bail!("descriptor '{}' is not supported in '{suffix}'", annotation.kind);
        }

        Ok(())
    }

    fn parse_known(pos: Pos, annotation: Annotation<'_>) -> anyhow::Result<Option<Element<'_>>> {
        if annotation.var.is_some() {
            return Ok(None);
        }

        if let Some(prop) = prop::v2::tra::Prop::parse(pos.clone(), annotation.kind, annotation.body)? {
            return Ok(Some(Element::Prop(prop::Prop::V2(prop::v2::Prop::Tra(prop)))));
        }

        if let Some(desc) = desc::v2::general::Desc::parse(pos.clone(), annotation.kind, annotation.body)? {
            return Ok(Some(Element::Desc(desc::v2::Desc::General(desc))));
        }

        if let Some(desc) = desc::v2::tra::parse(pos, annotation.kind, annotation.body.unwrap_or_default())? {
            return Ok(Some(Element::Desc(desc::v2::Desc::Timecontrol(desc))));
        }

        Ok(None)
    }
}

impl Spec for v0::Effect {
    fn is_known_prop(annotation: &Annotation<'_>) -> bool {
        annotation.var.is_none() && prop::v0::effect::is_kind(annotation.kind, annotation.body.is_some())
    }

    fn parse_known(pos: Pos, annotation: Annotation<'_>) -> anyhow::Result<Option<Element<'_>>> {
        if annotation.var.is_some() {
            return Ok(None);
        }

        Ok(prop::v0::effect::Prop::parse(pos, annotation.kind, annotation.body)?
            .map(|prop| Element::Prop(prop::Prop::V0(prop::v0::Prop::Effect(prop)))))
    }
}

impl Spec for v0::Scn {
    fn is_known_prop(annotation: &Annotation<'_>) -> bool {
        annotation.var.is_none()
            && prop::v0::effect::is_kind(annotation.kind, annotation.body.is_some())
            && !matches!(annotation.kind, "track2" | "track3")
    }

    fn parse_known(pos: Pos, annotation: Annotation<'_>) -> anyhow::Result<Option<Element<'_>>> {
        if matches!(annotation.kind, "track2" | "track3") {
            return Ok(None);
        }

        v0::Effect::parse_known(pos, annotation)
    }
}

impl Spec for v0::Tra {
    fn is_known_prop(annotation: &Annotation<'_>) -> bool {
        annotation.var.is_none() && prop::v0::tra::is_kind(annotation.kind, annotation.body.is_some())
    }

    fn parse_known(pos: Pos, annotation: Annotation<'_>) -> anyhow::Result<Option<Element<'_>>> {
        if annotation.var.is_some() {
            return Ok(None);
        }

        Ok(prop::v0::tra::Prop::parse(pos, annotation.kind, annotation.body)?
            .map(|prop| Element::Prop(prop::Prop::V0(prop::v0::Prop::Tra(prop)))))
    }
}

impl Spec for Unsupported {
    fn is_known_prop(_annotation: &Annotation<'_>) -> bool {
        false
    }

    fn parse_known(_pos: Pos, _annotation: Annotation<'_>) -> anyhow::Result<Option<Element<'_>>> {
        Ok(None)
    }
}

fn reject_v2_prop(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
    let has_var = annotation.var.is_some();
    let has_body = annotation.body.is_some();

    if prop::v2::effect::is_kind(annotation.kind, has_var, has_body)
        || (!has_var && prop::v2::tra::is_kind(annotation.kind, has_body))
    {
        bail!("property '{}' is not supported in '{suffix}'", annotation.kind);
    }

    Ok(())
}

fn reject_v2_desc(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<()> {
    if annotation.var.is_none()
        && (desc::v2::general::is_kind(annotation.kind, annotation.body.is_some())
            || desc::v2::anm::is_kind(annotation.kind)
            || desc::v2::tra::is_kind(annotation.kind))
    {
        bail!("descriptor '{}' is not supported in '{suffix}'", annotation.kind);
    }

    Ok(())
}

pub(in crate::build::exedit) fn is_known<F: Spec>(annotation: &Annotation<'_>) -> bool {
    F::is_known_prop(annotation) || F::is_known_desc(annotation)
}

pub(in crate::build::exedit) fn should_expand<F: Spec>(
    suffix: &str,
    annotation: &Annotation<'_>,
    is_inline: bool,
) -> anyhow::Result<bool> {
    if is_inline {
        if F::is_bindable(annotation) {
            return Ok(true);
        }

        if is_known::<F>(annotation) {
            return Ok(false);
        }
    } else if is_known::<F>(annotation) {
        return Ok(true);
    }

    F::reject_prop(suffix, annotation)?;
    F::reject_desc(suffix, annotation)?;
    Ok(false)
}

pub(in crate::build::exedit) fn parse<'a, F: Spec>(
    suffix: &str,
    pos: Pos,
    annotation: Annotation<'a>,
) -> anyhow::Result<Option<Element<'a>>> {
    if let Some(element) = F::parse_known(pos, annotation)? {
        return Ok(Some(element));
    }

    F::reject_prop(suffix, &annotation)?;
    F::reject_desc(suffix, &annotation)?;
    Ok(None)
}

macro_rules! dispatch {
    ($suffix:expr, |$F:ident| $body:expr) => {{
        let suffix = $suffix;
        if suffix.eq_ignore_ascii_case(".anm2") {
            type $F = crate::build::exedit::format::v2::Anm;
            $body
        } else if suffix.eq_ignore_ascii_case(".obj2")
            || suffix.eq_ignore_ascii_case(".cam2")
            || suffix.eq_ignore_ascii_case(".scn2")
        {
            type $F = crate::build::exedit::format::v2::Effect;
            $body
        } else if suffix.eq_ignore_ascii_case(".tra2") {
            type $F = crate::build::exedit::format::v2::Tra;
            $body
        } else if suffix.eq_ignore_ascii_case(".anm")
            || suffix.eq_ignore_ascii_case(".obj")
            || suffix.eq_ignore_ascii_case(".cam")
        {
            type $F = crate::build::exedit::format::v0::Effect;
            $body
        } else if suffix.eq_ignore_ascii_case(".scn") {
            type $F = crate::build::exedit::format::v0::Scn;
            $body
        } else if suffix.eq_ignore_ascii_case(".tra") {
            type $F = crate::build::exedit::format::v0::Tra;
            $body
        } else {
            type $F = crate::build::exedit::format::Unsupported;
            $body
        }
    }};
}

pub(super) use dispatch;

#[cfg(test)]
pub(in crate::build::exedit) fn is_prop(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<bool> {
    fn check<F: Spec>(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<bool> {
        if F::is_known_prop(annotation) {
            return Ok(true);
        }

        F::reject_prop(suffix, annotation)?;
        Ok(false)
    }

    dispatch!(suffix, |F| check::<F>(suffix, annotation))
}

#[cfg(test)]
pub(in crate::build::exedit) fn is_desc(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<bool> {
    fn check<F: Spec>(suffix: &str, annotation: &Annotation<'_>) -> anyhow::Result<bool> {
        if F::is_known_desc(annotation) {
            return Ok(true);
        }

        F::reject_desc(suffix, annotation)?;
        Ok(false)
    }

    dispatch!(suffix, |F| check::<F>(suffix, annotation))
}
