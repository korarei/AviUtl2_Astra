use crate::build::lang::hlsl;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::ops::Range;

pub(super) type Issue = (tracing::Level, hlsl::Span, String);

#[derive(Default)]
pub(super) struct Buffer {
    offset: Option<(usize, bool)>,
    numbers: Vec<Range<usize>>,
    prefix_ed: Option<usize>,
    has_explicit: bool,
    has_automatic: bool,
}

impl Buffer {
    pub(super) fn reset(&mut self) {
        self.offset = Some((0, false));
        self.numbers.clear();
        self.prefix_ed = None;
        self.has_explicit = false;
        self.has_automatic = false;
    }

    pub(super) fn invalidate(&mut self) {
        self.prefix_ed.get_or_insert(self.offset.map_or(0, |(ed, _)| ed));
        self.offset = None;
    }

    pub(super) fn place_member(
        &mut self,
        buffer: &str,
        span: hlsl::Span,
        member: &hlsl::Member<'_>,
        layout: &Layout,
        issues: &mut Vec<Issue>,
    ) {
        let annotation = member.annotation.filter(|annotation| annotation.name == "packoffset");
        let at = if let Some(annotation) = annotation {
            self.has_explicit = true;
            annotation
                .args
                .and_then(parse_pack_offset)
                .filter(|&at| at % layout.alignment == 0 && (layout.alignment >= 16 || at % 16 + layout.size <= 16))
        } else {
            self.has_automatic = true;
            self.offset.and_then(|(st, is_min_precision)| {
                let at = st.checked_add(layout.alignment - 1)? / layout.alignment * layout.alignment;
                if st % 16 != 0 && is_min_precision != layout.is_min_precision
                    || layout.alignment < 16 && at % 16 + layout.size > 16
                {
                    Some(at.checked_add(15)? / 16 * 16)
                } else {
                    Some(at)
                }
            })
        };

        let Some(at) = at.filter(|&at| at.checked_add(layout.size).is_some_and(|ed| ed <= 65_536)) else {
            self.invalidate();
            let kind = if annotation.is_some() {
                "unresolved packoffset"
            } else {
                "unresolved member offset"
            };
            issues.push((
                tracing::Level::DEBUG,
                span,
                format!("cannot determine layout of cbuffer '{buffer}': {kind}"),
            ));
            return;
        };

        let ed = at + layout.size;
        self.offset = Some((ed, layout.is_min_precision));
        self.numbers
            .extend(layout.numbers.iter().map(|range| at + range.start..at + range.end));

        if self.prefix_ed.is_some() && annotation.is_some() {
            issues.push((
                tracing::Level::INFO,
                span,
                format!(
                    "cbuffer '{buffer}' member '{}' has known byte range {at}..{ed}; surrounding layout is unknown",
                    member.name
                ),
            ));
        }
    }

    pub(super) fn report(&mut self, buffer: &str, span: hlsl::Span, issues: &mut Vec<Issue>) {
        if self.has_explicit && self.has_automatic {
            issues.push((
                tracing::Level::DEBUG,
                span,
                format!("cannot determine layout of cbuffer '{buffer}': mixed packoffset and automatic declarations"),
            ));
            return;
        }

        self.numbers.sort_unstable_by_key(|range| range.start);
        let limit = self
            .prefix_ed
            .unwrap_or_else(|| self.numbers.last().map_or(0, |range| range.end));
        let mut curr = 0;
        let mut has_padding = false;

        for range in self.numbers.iter().filter(|range| range.start < limit) {
            if range.start < curr {
                issues.push((
                    tracing::Level::DEBUG,
                    span,
                    format!("cannot determine number ordering in cbuffer '{buffer}': overlapping member locations"),
                ));
                return;
            }

            if range.start > curr {
                has_padding = true;
            }

            curr = curr.max(range.end.min(limit));
        }

        if !has_padding {
            return;
        }

        let mut pattern = String::new();
        let mut curr = 0;
        let push = |pattern: &mut String, label: &str, count: usize| {
            if count == 0 {
                return;
            }

            if !pattern.is_empty() {
                pattern.push_str(", ");
            }
            pattern.push_str(label);
            if count > 1 {
                write!(pattern, " x {count}").unwrap();
            }
        };

        for range in self.numbers.iter().filter(|range| range.start < limit) {
            if range.start > curr {
                push(&mut pattern, "padding(number)", (range.start - curr) / 4);
            }

            let ed = range.end.min(limit);

            if ed > curr {
                push(&mut pattern, "number", (ed - curr.max(range.start)) / 4);
                curr = ed;
            }
        }

        issues.push((
            tracing::Level::INFO,
            span,
            format!(
                "cbuffer '{buffer}' {}layout (number slots 1..{}): {pattern}",
                if self.prefix_ed.is_some() { "known prefix " } else { "" },
                limit / 4
            ),
        ));
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Layout {
    size: usize,
    alignment: usize,
    is_min_precision: bool,
    numbers: Vec<Range<usize>>,
}

pub(super) fn resolve_layout(ty: hlsl::Type<'_>, is_row_major: Option<bool>) -> Option<Layout> {
    match is_row_major {
        Some(is_row_major) => to_layout(ty, is_row_major),
        None if matches!(ty, hlsl::Type::Mat { rows, cols, .. } if rows != cols) => None,
        None => to_layout(ty, false),
    }
}

pub(super) fn apply_dims(
    base: &Layout,
    dims: impl DoubleEndedIterator<Item = Option<usize>>,
) -> Option<Cow<'_, Layout>> {
    dims.rev().try_fold(Cow::Borrowed(base), |layout, len| {
        let len = len?;
        let stride = layout.size.checked_add(15)? / 16 * 16;
        let size = stride.checked_mul(len.checked_sub(1)?)?.checked_add(layout.size)?;
        if size > 65_536 {
            return None;
        }

        let mut numbers = Vec::with_capacity(layout.numbers.len() * len);

        for index in 0..len {
            let st = stride * index;
            numbers.extend(layout.numbers.iter().map(|range| st + range.start..st + range.end));
        }

        Some(Cow::Owned(Layout {
            size,
            alignment: 16,
            is_min_precision: layout.is_min_precision,
            numbers,
        }))
    })
}

fn to_layout(ty: hlsl::Type<'_>, is_row_major: bool) -> Option<Layout> {
    let (scalar, len, width) = match ty {
        hlsl::Type::Scalar(scalar) => (scalar, 1, 1),
        hlsl::Type::Vec { scalar, cols } => (scalar, 1, cols),
        hlsl::Type::Mat { scalar, rows, cols } if is_row_major => (scalar, rows, cols),
        hlsl::Type::Mat { scalar, rows, cols } => (scalar, cols, rows),
        hlsl::Type::Named(_) => return None,
    };
    let width = width.checked_mul(scalar.size)?;
    let stride = width.checked_add(15)? / 16 * 16;
    Some(Layout {
        size: len.checked_sub(1)?.checked_mul(stride)?.checked_add(width)?,
        alignment: if len > 1 { 16 } else { scalar.size },
        is_min_precision: matches!(
            scalar.name,
            "min16float" | "min10float" | "min16int" | "min12int" | "min16uint"
        ),
        numbers: (0..len).map(|index| index * stride..index * stride + width).collect(),
    })
}

fn parse_pack_offset(src: &str) -> Option<usize> {
    let mut tokens = hlsl::Lexer::new(src).filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()));
    let hlsl::TokenKind::Ident(register) = tokens.next()?.ok()?.kind else {
        return None;
    };
    let register = register.strip_prefix('c')?.parse::<usize>().ok()?;
    let component = match tokens.next().transpose().ok()? {
        None => 0,
        Some(hlsl::Token {
            kind: hlsl::TokenKind::Dot,
            ..
        }) => {
            let hlsl::TokenKind::Ident(component) = tokens.next()?.ok()?.kind else {
                return None;
            };
            if tokens.next().is_some() {
                return None;
            }

            match component {
                "x" => 0,
                "y" => 4,
                "z" => 8,
                "w" => 12,
                _ => return None,
            }
        }
        _ => return None,
    };
    register
        .checked_mul(16)?
        .checked_add(component)
        .filter(|&at| at < 65_536)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packoffset_tokens() {
        for (src, expected) in [
            ("c0", Some(0)),
            ("c1.x", Some(16)),
            ("c1.y", Some(20)),
            (" c1 /* comment */ . y ", Some(20)),
            ("c2.y", Some(36)),
            ("c2.z", Some(40)),
            ("c2 /* gap */ . w", Some(44)),
            ("c4095.w", Some(65_532)),
            ("c4096", None),
            ("c", None),
            ("c-1", None),
            ("c0.x extra", None),
            ("c0..x", None),
            ("c0.q", None),
            ("c0.xy", None),
            ("c0.", None),
            ("c0.x /*", None),
            ("c0 /*", None),
            ("t0", None),
            ("", None),
        ] {
            assert_eq!(parse_pack_offset(src), expected, "{src}");
        }
    }
}
