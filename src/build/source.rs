use std::borrow::Cow;
use std::fmt;
use std::ops::{Bound, Range, RangeBounds};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
pub struct Map {
    fragments: Vec<Fragment>,
}

impl Map {
    pub fn new(text: impl Into<Arc<str>>, pos: Pos) -> Self {
        let text = text.into();
        let lines = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(st, _)| st + 1))
            .collect();
        let ed = text.len();
        let original = Arc::new(Original {
            pos,
            text: Arc::clone(&text),
            lines,
        });

        Self {
            fragments: vec![Fragment {
                text,
                span: 0..ed,
                original,
                src_span: None,
                dst_ed: ed,
            }],
        }
    }

    #[must_use]
    pub fn text(&self) -> Cow<'_, str> {
        let mut fragments = self.fragments.iter().filter(|fragment| !fragment.span.is_empty());
        let Some(first) = fragments.next() else {
            return Cow::Borrowed("");
        };

        let first = &first.text[first.span.clone()];
        let Some(second) = fragments.next() else {
            return Cow::Borrowed(first);
        };

        let mut dst = String::with_capacity(self.len());
        dst.push_str(first);

        for fragment in std::iter::once(second).chain(fragments) {
            dst.push_str(&fragment.text[fragment.span.clone()]);
        }

        Cow::Owned(dst)
    }

    #[must_use]
    pub fn locate(&self, pos: usize) -> Pos {
        let (fragment, span) = self.origin(pos);
        let pos = span.start;
        let Original { pos: base, text, lines } = fragment.original.as_ref();
        let line = lines.partition_point(|&st| st <= pos) - 1;

        Pos {
            file: Arc::clone(&base.file),
            line: base.line + line,
            col: text[lines[line]..pos].chars().count() + if line == 0 { base.col } else { 1 },
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.fragments.last().map_or(0, |fragment| fragment.dst_ed)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn copy(&mut self, src: &Self, span: impl RangeBounds<usize>) {
        let st = match span.start_bound() {
            Bound::Included(&st) => st,
            Bound::Excluded(&st) => st.checked_add(1).expect("source span start overflow"),
            Bound::Unbounded => 0,
        };
        let ed = match span.end_bound() {
            Bound::Included(&ed) => ed.checked_add(1).expect("source span end overflow"),
            Bound::Excluded(&ed) => ed,
            Bound::Unbounded => src.len(),
        };
        let span = st..ed;

        span.end
            .checked_sub(span.start)
            .filter(|_| span.end <= src.len())
            .expect("source span must be valid");

        if span.is_empty() {
            if !src.fragments.is_empty() {
                let (pos, offset) = src.find(span.start);
                let mut fragment = src.fragments[pos].clone();
                fragment.span = fragment.span.start + offset..fragment.span.start + offset;
                self.push(fragment);
            }

            return;
        }

        for (fragment, span) in src.parts(span) {
            let mut fragment = fragment.clone();
            fragment.span = span;
            self.push(fragment);
        }
    }

    pub(super) fn emit(&mut self, text: impl Into<Arc<str>>, src: &Self, span: Range<usize>) {
        span.end
            .checked_sub(span.start)
            .filter(|_| span.end <= src.len())
            .expect("source span must be valid");
        let (fragment, mut src_span) = src.origin(span.start);
        let original = Arc::clone(&fragment.original);

        if fragment.src_span.is_none() {
            let last_at = if span.is_empty() { span.end } else { span.end - 1 };
            let (pos, offset) = src.find(last_at);
            let last = &src.fragments[pos];
            src_span.end = if Arc::ptr_eq(&original, &last.original) {
                match &last.src_span {
                    Some(span) => span.end,
                    None => last.span.start + offset + usize::from(!span.is_empty()),
                }
            } else {
                src_span.start
            };
            src_span.end = src_span.end.max(src_span.start);
        }

        let text: Arc<str> = text.into();
        let ed = text.len();
        self.push(Fragment {
            text,
            span: 0..ed,
            original,
            src_span: Some(src_span),
            dst_ed: ed,
        });
    }

    pub(super) fn emit_at_end(&mut self, text: impl Into<Arc<str>>) {
        let (fragment, src_span) = self.origin(self.len());
        let original = Arc::clone(&fragment.original);
        let text: Arc<str> = text.into();
        let ed = text.len();
        self.push(Fragment {
            text,
            span: 0..ed,
            original,
            src_span: Some(src_span),
            dst_ed: ed,
        });
    }

    pub(super) fn slice(&self, span: impl RangeBounds<usize>) -> Self {
        let mut dst = Self::default();
        dst.copy(self, span);
        dst
    }

    pub(super) fn apply(&mut self, src: &Self, span: Range<usize>, edits: impl IntoIterator<Item = Edit>) {
        let mut curr = span.start;

        for Edit { span: edit, text } in edits {
            self.copy(src, curr..edit.start);
            curr = edit.end;

            if let Some(text) = text {
                self.emit(text, src, edit);
            }
        }

        self.copy(src, curr..span.end);
    }

    pub(super) fn indent(&self, padding: usize, indent: &str) -> Self {
        let mut dst = Self::default();
        let indent: Arc<str> = indent.into();
        let lines = self.lines();

        for (line, &st) in lines.iter().enumerate().take_while(|(_, st)| **st < self.len()) {
            let ed = lines.get(line + 1).copied().unwrap_or(self.len());

            if self.is_blank(st..ed) {
                dst.copy(self, st..ed);
            } else {
                dst.emit(Arc::clone(&indent), self, st..st + padding);
                dst.copy(self, st + padding..ed);
            }
        }

        if self.is_empty() {
            dst.copy(self, 0..0);
        }

        dst
    }

    pub(super) fn lines(&self) -> Cow<'_, [usize]> {
        if let [fragment] = self.fragments.as_slice()
            && fragment.src_span.is_none()
            && fragment.span == (0..fragment.text.len())
        {
            return Cow::Borrowed(&fragment.original.lines);
        }

        let mut lines = vec![0];

        let mut st = 0;
        for fragment in &self.fragments {
            lines.extend(
                fragment.text[fragment.span.clone()]
                    .match_indices('\n')
                    .map(|(at, _)| st + at + 1),
            );
            st = fragment.dst_ed;
        }

        Cow::Owned(lines)
    }

    pub(super) fn is_blank(&self, span: Range<usize>) -> bool {
        span.is_empty()
            || self
                .parts(span)
                .all(|(fragment, span)| fragment.text[span].chars().all(char::is_whitespace))
    }

    pub(super) fn ends_with(&self, ch: char) -> bool {
        self.fragments
            .iter()
            .rev()
            .find(|fragment| !fragment.span.is_empty())
            .is_some_and(|fragment| fragment.text[fragment.span.clone()].ends_with(ch))
    }

    fn find(&self, pos: usize) -> (usize, usize) {
        self.len().checked_sub(pos).expect("source offset must be valid");
        let index = self.fragments.partition_point(|fragment| fragment.dst_ed <= pos);
        let index = index.min(
            self.fragments
                .len()
                .checked_sub(1)
                .expect("source has no original position"),
        );
        (
            index,
            pos - index.checked_sub(1).map_or(0, |index| self.fragments[index].dst_ed),
        )
    }

    fn origin(&self, pos: usize) -> (&Fragment, Range<usize>) {
        let (index, offset) = self.find(pos);
        let fragment = &self.fragments[index];
        let span = fragment.src_span.clone().unwrap_or_else(|| {
            let pos = fragment.span.start + offset;
            pos..pos
        });

        (fragment, span)
    }

    fn parts(&self, span: Range<usize>) -> impl Iterator<Item = (&Fragment, Range<usize>)> {
        let (pos, _) = self.find(span.start);
        let st = pos.checked_sub(1).map_or(0, |pos| self.fragments[pos].dst_ed);
        self.fragments[pos..].iter().scan(st, move |st, fragment| {
            let curr = *st;
            *st = fragment.dst_ed;

            if curr >= span.end {
                return None;
            }

            let offset = span.start.saturating_sub(curr);
            let len = fragment.dst_ed.min(span.end) - (curr + offset);
            Some((
                fragment,
                fragment.span.start + offset..fragment.span.start + offset + len,
            ))
        })
    }

    fn push(&mut self, mut fragment: Fragment) {
        let len = fragment.span.len();

        if len == 0 {
            fragment.dst_ed = self.len();

            if let Some(prev) = self.fragments.last_mut()
                && prev.span.is_empty()
            {
                *prev = fragment;
            } else {
                self.fragments.push(fragment);
            }

            return;
        }

        fragment.dst_ed = self.len() + len;

        if self.fragments.last().is_some_and(|fragment| fragment.span.is_empty()) {
            let _ = self.fragments.pop();
        }

        if let Some(prev) = self.fragments.last_mut()
            && Arc::ptr_eq(&prev.text, &fragment.text)
            && prev.span.end == fragment.span.start
            && Arc::ptr_eq(&prev.original, &fragment.original)
            && prev.src_span == fragment.src_span
        {
            prev.span.end = fragment.span.end;
            prev.dst_ed = fragment.dst_ed;
        } else {
            self.fragments.push(fragment);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pos {
    pub file: Arc<Path>,
    pub line: usize,
    pub col: usize,
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.col)
    }
}

pub(super) struct Edit {
    pub(super) span: Range<usize>,
    pub(super) text: Option<Arc<str>>,
}

#[derive(Debug, Clone)]
struct Fragment {
    text: Arc<str>,
    span: Range<usize>,
    original: Arc<Original>,
    src_span: Option<Range<usize>>,
    dst_ed: usize,
}

#[derive(Debug)]
struct Original {
    pos: Pos,
    text: Arc<str>,
    lines: Vec<usize>,
}

pub(super) fn resolve_include(path: &str, is_quoted: bool, pos: &Pos, dirs: &[PathBuf]) -> anyhow::Result<PathBuf> {
    let dir = pos.file.parent().unwrap_or(Path::new("."));
    is_quoted
        .then(|| dir.join(path))
        .filter(|file| file.is_file())
        .or_else(|| dirs.iter().map(|dir| dir.join(path)).find(|file| file.is_file()))
        .ok_or_else(|| anyhow::anyhow!("{pos}: include file '{path}' not found from '{}'", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::lang::astra;

    fn make_map(text: &str, file: &str) -> Map {
        Map::new(
            text,
            Pos {
                file: Path::new(file).into(),
                line: 1,
                col: 1,
            },
        )
    }

    #[test]
    fn ordered_edits() {
        let src = make_map("abcd", "input.txt");
        let mut dst = Map::default();
        dst.apply(
            &src,
            0..src.len(),
            [
                Edit { span: 1..2, text: None },
                Edit {
                    span: 2..3,
                    text: Some("XY".into()),
                },
            ],
        );
        assert_eq!(dst.text(), "aXYd");
        let (fragment, span) = dst.origin(1);
        assert_eq!((fragment.original.text.as_ref(), span), ("abcd", 2..3));
        assert_eq!(dst.locate(dst.len()).to_string(), "input.txt:1:5");
    }

    #[test]
    fn preserve_origins() {
        let text = "あ${X}";
        let src = Map::new(
            text,
            Pos {
                file: Path::new("test.lua").into(),
                line: 1,
                col: 1,
            },
        );
        let vars = indexmap::IndexMap::from([("X".to_owned(), String::new())]);
        let mut map = Map::default();
        map.apply(
            &src,
            0..src.len(),
            super::super::lang::astra::expand(text, &vars, astra::Escape::None)
                .unwrap()
                .to_edits(0),
        );
        assert_eq!(map.text(), "あ");
        assert_eq!(map.locate(map.len()).to_string(), "test.lua:1:6");

        let vars = indexmap::IndexMap::from([("X".to_owned(), "日本語\nsecond".to_owned())]);
        let mut map = Map::default();
        map.apply(
            &src,
            0..src.len(),
            super::super::lang::astra::expand(text, &vars, astra::Escape::None)
                .unwrap()
                .to_edits(0),
        );
        assert_eq!(map.locate(13).to_string(), "test.lua:1:2");
        let (fragment, span) = map.origin(13);
        assert_eq!((fragment.original.text.as_ref(), span), (text, 3..7));
        assert_eq!(map.locate(map.len()).to_string(), "test.lua:1:6");
    }

    #[test]
    fn slice_ranges() {
        let src = make_map("あb\nc", "input.lua");

        for (map, expected, pos) in [
            (src.slice(..), "あb\nc", "input.lua:1:1"),
            (src.slice(..=3), "あb", "input.lua:1:1"),
            (src.slice(3..), "b\nc", "input.lua:1:2"),
            (src.slice(3..=3), "b", "input.lua:1:2"),
            (src.slice(src.len()..), "", "input.lua:2:2"),
        ] {
            assert_eq!(map.text(), expected);
            assert_eq!(map.locate(0).to_string(), pos);
        }

        let mut dst = Map::default();
        dst.copy(&src, (Bound::Excluded(2), Bound::Included(3)));
        assert_eq!(dst.text(), "b");
        assert_eq!(dst.locate(0).to_string(), "input.lua:1:2");
        assert!(Map::default().slice(..).is_empty());
    }

    #[test]
    fn preserve_expansion_edits_in_slices() {
        let text = "headあ$${X}:${X}tail";
        let src = make_map(text, "input.lua");
        let vars = indexmap::IndexMap::from([("X".to_owned(), "値\n次".to_owned())]);
        let span = 4..text.len() - 4;
        let mut map = src.slice(0..span.start);
        map.apply(
            &src,
            span.clone(),
            super::super::lang::astra::expand(&text[span.clone()], &vars, astra::Escape::None)
                .unwrap()
                .to_edits(span.start),
        );
        map.copy(&src, span.end..src.len());

        let text = map.text();
        assert_eq!(text, "headあ${X}:値\n次tail");
        assert_eq!(map.locate(text.find('$').unwrap()).to_string(), "input.lua:1:7");
        assert_eq!(map.locate(text.find('値').unwrap()).to_string(), "input.lua:1:12");
        assert_eq!(map.locate(text.find('次').unwrap()).to_string(), "input.lua:1:12");
        assert_eq!(map.locate(text.find("tail").unwrap()).to_string(), "input.lua:1:16");
    }

    #[test]
    fn concatenate_origins() {
        let first = make_map("first\n", "first.lua");
        let second = make_map("second\n", "second.lua");
        let mut map = first.clone();
        map.copy(&second, ..);
        assert_eq!(map.text(), "first\nsecond\n");
        assert_eq!(map.locate(0).to_string(), "first.lua:1:1");
        assert_eq!(map.locate(first.len()).to_string(), "second.lua:1:1");
        assert_eq!(map.locate(map.len()).to_string(), "second.lua:2:1");
    }

    #[test]
    fn share_fragments() {
        let src = make_map("あb\nう", "original.lua");
        let mut map = src.slice(0..3);
        map.copy(&src, 3..src.len());
        assert_eq!(map.text(), "あb\nう");
        assert_eq!(map.fragments.len(), 1);
        assert!(Arc::ptr_eq(&src.fragments[0].text, &map.fragments[0].text));
        assert_eq!(map.locate(5).to_string(), "original.lua:2:1");
        assert_eq!(map.locate(8).to_string(), "original.lua:2:2");
    }

    #[test]
    fn preserve_replacements() {
        let src = make_map("あ${X}後", "original.lua");
        let mut map = src.slice(0..3);
        map.emit("first\nsecond", &src, 3..7);
        map.copy(&src, 7..src.len());
        let map = map.slice(5..map.len());
        assert_eq!(map.text(), "rst\nsecond後");
        assert_eq!(map.locate(4).to_string(), "original.lua:1:2");
        let (fragment, span) = map.origin(4);
        assert_eq!((fragment.original.text.as_ref(), span), ("あ${X}後", 3..7));
        assert_eq!(map.locate(10).to_string(), "original.lua:1:6");
        let (fragment, span) = map.origin(10);
        assert_eq!((fragment.original.text.as_ref(), span), ("あ${X}後", 7..7));
    }

    #[test]
    fn preserve_files() {
        let src = make_map("  return あ\n\n", "included.lua");
        let body = src.indent(2, "    ");
        let caller = make_map("require('included')", "main.lua");
        let mut map = Map::default();
        map.emit("(function()\n", &caller, 0..caller.len());
        let body_st = map.len();
        map.copy(&body, 0..body.len());
        map.emit("end)()", &caller, 0..caller.len());
        assert_eq!(map.locate(0).to_string(), "main.lua:1:1");
        assert_eq!(map.locate(body_st).to_string(), "included.lua:1:1");
        assert_eq!(map.locate(body_st + 4).to_string(), "included.lua:1:3");
        assert_eq!(map.locate(body_st + 11).to_string(), "included.lua:1:10");
        assert_eq!(map.locate(body_st + body.len()).to_string(), "main.lua:1:1");
    }

    #[test]
    fn preserve_empty_origins() {
        let src = make_map("", "empty.lua");
        assert_eq!(src.slice(0..0).locate(0).to_string(), "empty.lua:1:1");
        let src = make_map("あ\n", "original.lua");
        let empty = src.slice(src.len()..src.len());
        assert_eq!(empty.locate(0).to_string(), "original.lua:2:1");
        let mut map = Map::default();
        map.emit("wrapper", &empty, 0..0);
        assert_eq!(map.locate(map.len()).to_string(), "original.lua:2:1");
    }

    #[test]
    fn preserve_generated_origins() {
        let src = make_map("ab\ncd", "original.txt");
        let mut map = Map::default();
        map.emit(Arc::clone(&src.fragments[0].text), &src, 0..2);
        assert!(Arc::ptr_eq(&src.fragments[0].text, &map.fragments[0].text));
        assert_eq!(map.locate(4).to_string(), "original.txt:1:1");
        let (fragment, span) = map.origin(4);
        assert_eq!((fragment.original.text.as_ref(), span), ("ab\ncd", 0..2));
    }

    #[test]
    fn locate_boundaries() {
        let src = Map::new(
            "あ\nb\n",
            Pos {
                file: Path::new("input.lua").into(),
                line: 7,
                col: 4,
            },
        );

        for (at, expected) in [
            (0, "input.lua:7:4"),
            (3, "input.lua:7:5"),
            (4, "input.lua:8:1"),
            (5, "input.lua:8:2"),
            (6, "input.lua:9:1"),
        ] {
            assert_eq!(src.locate(at).to_string(), expected);
        }
        assert_eq!(src.lines().as_ref(), [0, 4, 6]);
        assert!(matches!(src.text(), Cow::Borrowed(_)));
        assert!(matches!(src.lines(), Cow::Borrowed(_)));
    }

    #[test]
    fn preserve_fragment_boundaries() {
        let first = make_map("first\n", "first.lua");
        let second = make_map("second", "second.lua");
        let mut map = Map::default();
        map.copy(&first, 0..first.len());
        map.copy(&second, 0..second.len());
        assert_eq!(map.text(), "first\nsecond");
        assert_eq!(map.lines().as_ref(), [0, 6]);
        assert_eq!(map.locate(5).to_string(), "first.lua:1:6");
        assert_eq!(map.locate(6).to_string(), "second.lua:1:1");
        assert_eq!(map.locate(map.len()).to_string(), "second.lua:1:7");
        assert!(matches!(map.text(), Cow::Owned(_)));
        assert!(map.ends_with('d'));
        assert!(!map.ends_with('\n'));
    }

    #[test]
    fn preserve_nested_replacements() {
        let src = make_map("aVALUEz", "input.lua");
        let mut map = src.slice(0..1);
        map.emit("first\nsecond", &src, 1..6);
        map.copy(&src, 6..7);
        let mut dst = map.slice(0..3);
        dst.emit("replacement", &map, 3..9);
        dst.copy(&map, 9..map.len());
        assert_eq!(dst.locate(3).to_string(), "input.lua:1:2");
        let (fragment, span) = dst.origin(3);
        assert_eq!((fragment.original.text.as_ref(), span), ("aVALUEz", 1..6));
        assert_eq!(dst.locate(dst.len()).to_string(), "input.lua:1:8");
    }

    #[test]
    fn preserve_blank_lines() {
        let src = make_map("  first\n \t\n  second", "input.lua");
        assert!(src.is_blank(8..11));
        assert!(!src.is_blank(0..8));
        let map = src.indent(2, "    ");
        assert_eq!(map.text(), "    first\n \t\n    second");
        let text = map.text();
        assert_eq!(map.locate(text.find("first").unwrap()).to_string(), "input.lua:1:3");
        assert_eq!(map.locate(text.find("second").unwrap()).to_string(), "input.lua:3:3");
        assert!(Map::default().is_empty());
        assert!(!Map::default().ends_with('\n'));
        assert_eq!(
            make_map("", "input.lua").indent(0, "").locate(0).to_string(),
            "input.lua:1:1"
        );
    }
}
