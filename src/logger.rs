use std::fmt::{self, Write};
use tracing::{Event, Level};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

#[derive(Debug, Clone, Default)]
pub struct Formatter;

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, val: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{val:?}");
        } else {
            self.fields.push((field.name().to_string(), format!("{val:?}")));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, val: &str) {
        if field.name() == "message" {
            self.message.push_str(val);
        } else {
            self.fields.push((field.name().to_string(), val.to_string()));
        }
    }
}

impl<C, N> FormatEvent<C, N> for Formatter
where
    C: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, _ctx: &FmtContext<'_, C, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let (status, color, text): (&str, &str, &str) = match *event.metadata().level() {
            Level::ERROR => ("error:", "1;31", visitor.message.as_str()),
            Level::WARN => ("warning:", "1;33", visitor.message.as_str()),
            Level::DEBUG => ("debug:", "2", visitor.message.as_str()),
            Level::TRACE => ("trace:", "2", visitor.message.as_str()),
            Level::INFO => parse_info_message(&visitor.message),
        };

        if writer.has_ansi_escapes() {
            write!(writer, "\x1b[{color}m{status:>12}\x1b[0m ")?;
        } else {
            write!(writer, "{status:>12} ")?;
        }

        let mut lines = text.lines();
        if let Some(first) = lines.next() {
            write!(writer, "{first}")?;
            if !visitor.fields.is_empty() {
                for (k, v) in &visitor.fields {
                    write!(writer, " {k}={v}")?;
                }
            }

            writeln!(writer)?;
        } else {
            writeln!(writer)?;
        }

        for line in lines {
            if writer.has_ansi_escapes() && line == "Next steps:" {
                writeln!(writer, "{:>12} \x1b[1;36m{line}\x1b[0m", "")?;
            } else {
                writeln!(writer, "{:>12} {line}", "")?;
            }
        }

        Ok(())
    }
}

fn parse_info_message(msg: &str) -> (&str, &str, &str) {
    static KNOWN_STATUSES: [&str; 24] = [
        "Archiving",
        "Blocking",
        "Building",
        "Checking",
        "Cleaned",
        "Compiling",
        "Downloading",
        "Extracting",
        "Finished",
        "Initialized",
        "Installing",
        "Packaging",
        "Preparing",
        "Refreshed",
        "Releasing",
        "Reloading",
        "Removed",
        "Removing",
        "Requesting",
        "Running",
        "Set",
        "Stopping",
        "Updating",
        "Writing",
    ];

    if msg.starts_with("$ ") {
        return ("Running", "1;32", msg);
    }

    if let Some((first_word, rest)) = msg.split_once(' ')
        && let Some(&status) = KNOWN_STATUSES.iter().find(|&&s| s == first_word)
    {
        let color = match status {
            "Building" | "Compiling" | "Blocking" | "Releasing" | "Reloading" | "Stopping" => "1;36",
            _ => "1;32",
        };
        return (status, color, rest);
    }

    ("Info", "1;32", msg)
}

#[cfg(test)]
mod tests {
    use super::parse_info_message;

    #[test]
    fn classifies_info_messages() {
        for (message, expected) in [
            ("$ cargo build", ("Running", "1;32", "$ cargo build")),
            ("Building 'main'", ("Building", "1;36", "'main'")),
            ("Downloading package", ("Downloading", "1;32", "package")),
            ("message", ("Info", "1;32", "message")),
        ] {
            assert_eq!(parse_info_message(message), expected);
        }
    }
}
