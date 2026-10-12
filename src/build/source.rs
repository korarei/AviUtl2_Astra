use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
pub(super) struct Map {
    fragments: Vec<Fragment>,
}

impl Map {
    pub(super) fn new(text: impl Into<Arc<str>>, pos: Position) -> Self {
        let text = text.into();
        let lines = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(st, _)| st + 1))
            .collect();
        let ed = text.len();
        let original = Arc::new((pos, Arc::clone(&text), lines));
        Self {
            fragments: vec![Fragment {
                text,
                span: 0..ed,
                original,
                origin: 0..ed,
                ed,
            }],
        }
    }

    pub(super) fn text(&self) -> Cow<'_, str> {
        let mut fragments = self.fragments.iter().filter(|fragment| !fragment.span.is_empty());
        let Some(first) = fragments.next() else {
            return Cow::Borrowed("");
        };

        let first = &first.text[first.span.clone()];
        let Some(second) = fragments.next() else {
            return Cow::Borrowed(first);
        };

        let mut result = String::with_capacity(self.len());
        result.push_str(first);

        for fragment in std::iter::once(second).chain(fragments) {
            result.push_str(&fragment.text[fragment.span.clone()]);
        }

        Cow::Owned(result)
    }

    pub(super) fn locate(&self, at: usize) -> Position {
        let (original, span) = self.resolve(at);
        let at = span.start;
        let (pos, text, lines) = original.as_ref();
        let line = lines.partition_point(|&st| st <= at) - 1;
        Position {
            file: Arc::clone(&pos.file),
            line: pos.line + line,
            col: text[lines[line]..at].chars().count() + if line == 0 { pos.col } else { 1 },
        }
    }

    pub(super) fn lines(&self) -> Cow<'_, [usize]> {
        if let [fragment] = self.fragments.as_slice()
            && Arc::ptr_eq(&fragment.text, &fragment.original.1)
            && fragment.span == (0..fragment.text.len())
        {
            return Cow::Borrowed(&fragment.original.2);
        }

        let mut result = vec![0];

        let mut st = 0;
        for fragment in &self.fragments {
            result.extend(
                fragment.text[fragment.span.clone()]
                    .match_indices('\n')
                    .map(|(at, _)| st + at + 1),
            );
            st = fragment.ed;
        }

        Cow::Owned(result)
    }

    pub(super) fn original(&self, at: usize) -> (&str, Range<usize>) {
        let (original, span) = self.resolve(at);
        (&original.1, span)
    }

    pub(super) fn append(&mut self, src: &Self, span: Range<usize>) {
        span.end
            .checked_sub(span.start)
            .filter(|_| span.end <= src.len())
            .expect("source span must be valid");

        if span.is_empty() {
            if !src.fragments.is_empty() {
                let (i, offset) = src.find(span.start);
                let mut fragment = src.fragments[i].clone();
                fragment.span = fragment.span.start + offset..fragment.span.start + offset;
                if Arc::ptr_eq(&fragment.text, &fragment.original.1) {
                    fragment.origin = fragment.origin.start + offset..fragment.origin.start + offset;
                }

                self.push(fragment);
            }

            return;
        }

        for (fragment, span) in src.parts(span) {
            let offset = span.start - fragment.span.start;
            let mut fragment = fragment.clone();
            fragment.span = span;
            if Arc::ptr_eq(&fragment.text, &fragment.original.1) {
                fragment.origin = fragment.origin.start + offset..fragment.origin.start + offset + fragment.span.len();
            }

            self.push(fragment);
        }
    }

    pub(super) fn generate(&mut self, text: impl Into<Arc<str>>, src: &Self, span: Range<usize>) {
        span.end
            .checked_sub(span.start)
            .filter(|_| span.end <= src.len())
            .expect("source span must be valid");
        let (i, _) = src.find(span.start);
        let fragment = &src.fragments[i];
        let (original, mut origin) = (Arc::clone(&fragment.original), src.original(span.start).1);

        if Arc::ptr_eq(&fragment.text, &fragment.original.1) {
            let last_at = if span.is_empty() { span.end } else { span.end - 1 };
            let (ed, offset) = src.find(last_at);
            let last = &src.fragments[ed];
            origin.end = if Arc::ptr_eq(&original, &last.original) {
                if Arc::ptr_eq(&last.text, &last.original.1) {
                    last.origin.start + offset + usize::from(!span.is_empty())
                } else {
                    last.origin.end
                }
            } else {
                origin.start
            };
            origin.end = origin.end.max(origin.start);
        }

        let text: Arc<str> = text.into();
        let text = if Arc::ptr_eq(&text, &original.1) {
            Arc::from(text.as_ref())
        } else {
            text
        };
        let ed = text.len();
        self.push(Fragment {
            text,
            span: 0..ed,
            original,
            origin,
            ed,
        });
    }

    pub(super) fn slice(&self, span: Range<usize>) -> Self {
        let mut result = Self::default();
        result.append(self, span);
        result
    }

    pub(super) fn indent(&self, padding: usize, indent: &str) -> Self {
        let mut result = Self::default();
        let indent: Arc<str> = indent.into();
        let lines = self.lines();

        for (i, &st) in lines.iter().enumerate().take_while(|(_, st)| **st < self.len()) {
            let ed = lines.get(i + 1).copied().unwrap_or(self.len());

            if self.is_blank(st..ed) {
                result.append(self, st..ed);
            } else {
                result.generate(Arc::clone(&indent), self, st..st + padding);
                result.append(self, st + padding..ed);
            }
        }

        if self.is_empty() {
            result.append(self, 0..0);
        }

        result
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

    pub(super) fn len(&self) -> usize {
        self.fragments.last().map_or(0, |fragment| fragment.ed)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn find(&self, at: usize) -> (usize, usize) {
        self.len().checked_sub(at).expect("source offset must be valid");
        let i = self.fragments.partition_point(|fragment| fragment.ed <= at);
        let i = i.min(
            self.fragments
                .len()
                .checked_sub(1)
                .expect("source has no original position"),
        );
        (i, at - i.checked_sub(1).map_or(0, |i| self.fragments[i].ed))
    }

    fn resolve(&self, at: usize) -> (&Original, Range<usize>) {
        let (i, offset) = self.find(at);
        let fragment = &self.fragments[i];
        let span = if Arc::ptr_eq(&fragment.text, &fragment.original.1) {
            let st = fragment.origin.start + offset;
            st..st
        } else {
            fragment.origin.clone()
        };

        (&fragment.original, span)
    }

    fn push(&mut self, mut fragment: Fragment) {
        let len = fragment.span.len();

        if len == 0 {
            fragment.ed = self.len();

            if let Some(last) = self.fragments.last_mut()
                && last.span.is_empty()
            {
                *last = fragment;
            } else {
                self.fragments.push(fragment);
            }

            return;
        }

        fragment.ed = self.len() + len;

        if self.fragments.last().is_some_and(|fragment| fragment.span.is_empty()) {
            let _ = self.fragments.pop();
        }

        if let Some(prev) = self.fragments.last_mut()
            && Arc::ptr_eq(&prev.text, &fragment.text)
            && prev.span.end == fragment.span.start
            && Arc::ptr_eq(&prev.original, &fragment.original)
            && if Arc::ptr_eq(&prev.text, &prev.original.1) {
                prev.origin.end == fragment.origin.start
            } else {
                prev.origin == fragment.origin
            }
        {
            prev.span.end = fragment.span.end;
            if Arc::ptr_eq(&prev.text, &prev.original.1) {
                prev.origin.end = fragment.origin.end;
            }
            prev.ed = fragment.ed;
        } else {
            if self.is_empty() {
                self.fragments.clear();
            }

            self.fragments.push(fragment);
        }
    }

    fn parts(&self, span: Range<usize>) -> impl Iterator<Item = (&Fragment, Range<usize>)> {
        let (i, _) = self.find(span.start);
        let st = i.checked_sub(1).map_or(0, |i| self.fragments[i].ed);
        self.fragments[i..].iter().scan(st, move |st, fragment| {
            let curr = *st;
            *st = fragment.ed;

            if curr >= span.end {
                return None;
            }

            let offset = span.start.saturating_sub(curr);
            let len = fragment.ed.min(span.end) - (curr + offset);
            Some((
                fragment,
                fragment.span.start + offset..fragment.span.start + offset + len,
            ))
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct Position {
    pub(super) file: Arc<Path>,
    pub(super) line: usize,
    pub(super) col: usize,
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.col)
    }
}

#[derive(Debug, Clone)]
struct Fragment {
    text: Arc<str>,
    span: Range<usize>,
    original: Original,
    origin: Range<usize>,
    ed: usize,
}

type Original = Arc<(Position, Arc<str>, Vec<usize>)>;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_map(text: &str, file: &str) -> Map {
        Map::new(
            text,
            Position {
                file: Path::new(file).into(),
                line: 1,
                col: 1,
            },
        )
    }

    #[test]
    fn share_fragments() {
        let src = make_map("あb\nう", "original.lua");
        let mut map = src.slice(0..3);
        map.append(&src, 3..src.len());
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
        map.generate("first\nsecond", &src, 3..7);
        map.append(&src, 7..src.len());
        let map = map.slice(5..map.len());
        assert_eq!(map.text(), "rst\nsecond後");
        assert_eq!(map.locate(4).to_string(), "original.lua:1:2");
        assert_eq!(map.original(4), ("あ${X}後", 3..7));
        assert_eq!(map.locate(10).to_string(), "original.lua:1:6");
        assert_eq!(map.original(10), ("あ${X}後", 7..7));
    }

    #[test]
    fn preserve_files() {
        let src = make_map("  return あ\n\n", "included.lua");
        let body = src.indent(2, "    ");
        let caller = make_map("require('included')", "main.lua");
        let mut map = Map::default();
        map.generate("(function()\n", &caller, 0..caller.len());
        let body_st = map.len();
        map.append(&body, 0..body.len());
        map.generate("end)()", &caller, 0..caller.len());
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
        map.generate("wrapper", &empty, 0..0);
        assert_eq!(map.locate(map.len()).to_string(), "original.lua:2:1");
    }

    #[test]
    fn preserve_generated_origins() {
        let src = make_map("ab\ncd", "original.txt");
        let mut map = Map::default();
        map.generate(Arc::clone(&src.fragments[0].text), &src, 0..2);
        assert_eq!(map.locate(4).to_string(), "original.txt:1:1");
        assert_eq!(map.original(4), ("ab\ncd", 0..2));
    }

    #[test]
    fn locate_boundaries() {
        let src = Map::new(
            "あ\nb\n",
            Position {
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
        map.append(&first, 0..first.len());
        map.append(&second, 0..second.len());
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
        map.generate("first\nsecond", &src, 1..6);
        map.append(&src, 6..7);
        let mut nested = map.slice(0..3);
        nested.generate("replacement", &map, 3..9);
        nested.append(&map, 9..map.len());
        assert_eq!(nested.locate(3).to_string(), "input.lua:1:2");
        assert_eq!(nested.original(3), ("aVALUEz", 1..6));
        assert_eq!(nested.locate(nested.len()).to_string(), "input.lua:1:8");
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
