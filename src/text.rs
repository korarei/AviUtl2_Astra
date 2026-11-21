use std::borrow::Cow;

pub(crate) fn normalize_newlines<'a>(src: impl Into<Cow<'a, str>>) -> Cow<'a, str> {
    let src = src.into();

    if !src.contains('\r') {
        return src;
    }

    let mut dst = String::with_capacity(src.len());
    let mut rest = src.as_ref();

    while let Some(at) = rest.find('\r') {
        dst.push_str(&rest[..at]);
        dst.push('\n');
        let skip = usize::from(rest.as_bytes().get(at + 1) == Some(&b'\n')) + 1;
        rest = &rest[at + skip..];
    }

    dst.push_str(rest);
    Cow::Owned(dst)
}
