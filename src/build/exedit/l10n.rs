use indexmap::IndexSet;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(super) enum Format {
    None,
    Basic,
    Full,
}

impl Format {
    pub(super) fn collector(self) -> Collector {
        match self {
            Self::None => Collector::None,
            Self::Basic => Collector::Basic {
                labels: BTreeSet::new(),
            },
            Self::Full => Collector::Full {
                names: IndexSet::new(),
                labels: BTreeSet::new(),
            },
        }
    }
}

pub(super) enum Collector {
    None,
    Basic {
        labels: BTreeSet<String>,
    },
    Full {
        names: IndexSet<String>,
        labels: BTreeSet<String>,
    },
}

impl Collector {
    pub(super) fn push(&mut self, names: &[&str], labels: &[&str]) {
        match self {
            Self::None => {}
            Self::Basic { labels: dst } => {
                for &label in labels {
                    if !dst.contains(label) {
                        dst.insert(label.to_owned());
                    }
                }
            }
            Self::Full {
                names: dst_names,
                labels: dst_labels,
            } => {
                for &name in names {
                    if !dst_names.contains(name) {
                        dst_names.insert(name.to_owned());
                    }
                }
                for &label in labels {
                    if !dst_labels.contains(label) {
                        dst_labels.insert(label.to_owned());
                    }
                }
            }
        }
    }

    pub(super) fn render(self, key: &str) -> String {
        match self {
            Self::None => String::new(),
            Self::Basic { labels } => render_sections(key, &labels, None),
            Self::Full { names, labels } => render_sections(key, &labels, Some(&names)),
        }
    }
}

fn render_sections(key: &str, labels: &BTreeSet<String>, names: Option<&IndexSet<String>>) -> String {
    let mut cap = "[".len() + key.len() + "]\n".len() + key.len() + "=\n".len() + "\n".len();

    for label in labels {
        if label != key {
            cap += label.len() + "=\n".len();
        }
    }

    if let Some(names) = names {
        cap += "[Tips.".len() + key.len() + "]\neffect.name=\n".len() + "\n".len();

        for name in names {
            cap += name.len() + "=\n".len();
        }
    }

    let mut dst = String::with_capacity(cap);
    dst.push('[');
    dst.push_str(key);
    dst.push_str("]\n");
    dst.push_str(key);
    dst.push_str("=\n");

    for label in labels {
        if label != key {
            dst.push_str(label);
            dst.push_str("=\n");
        }
    }

    dst.push('\n');

    if let Some(names) = names {
        dst.push_str("[Tips.");
        dst.push_str(key);
        dst.push_str("]\neffect.name=\n");

        for name in names {
            dst.push_str(name);
            dst.push_str("=\n");
        }

        dst.push('\n');
    }

    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_collected_names_and_labels() {
        let mut full = Format::Full.collector();
        full.push(&["Z"], &["Z"]);
        full.push(&["A"], &["A", "effect"]);
        full.push(&["Z"], &["A"]);
        assert_eq!(
            full.render("effect"),
            "[effect]\neffect=\nA=\nZ=\n\n[Tips.effect]\neffect.name=\nZ=\nA=\n\n"
        );

        let mut basic = Format::Basic.collector();
        basic.push(&["Z", "A", "Z"], &["Z", "A", "effect", "A"]);
        assert_eq!(basic.render("effect"), "[effect]\neffect=\nA=\nZ=\n\n");

        let mut none = Format::None.collector();
        none.push(&["Z", "A"], &["Z", "A"]);
        assert_eq!(none.render("effect"), "");
    }
}
