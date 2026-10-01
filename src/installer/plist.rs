//! The small part of Apple's XML property list format that LaunchAgents use:
//! dictionaries (in insertion order), arrays, strings, integers, booleans.

#[derive(Debug, Clone, PartialEq)]
pub enum Plist {
    Dict(Vec<(String, Plist)>),
    Array(Vec<Plist>),
    String(String),
    Integer(i64),
    Bool(bool),
}

impl Plist {
    pub fn dict(entries: Vec<(&str, Plist)>) -> Plist {
        Plist::Dict(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    pub fn strings<I: IntoIterator<Item = S>, S: Into<String>>(values: I) -> Plist {
        Plist::Array(
            values
                .into_iter()
                .map(|v| Plist::String(v.into()))
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Plist::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn to_xml(&self) -> String {
        let mut out = String::from(concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\">\n"
        ));
        self.write(&mut out, 0);
        out.push_str("</plist>\n");
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let indent = "\t".repeat(depth);
        match self {
            Plist::Dict(entries) => {
                out.push_str(&format!("{indent}<dict>\n"));
                for (key, value) in entries {
                    out.push_str(&format!("{indent}\t<key>{}</key>\n", escape(key)));
                    value.write(out, depth + 1);
                }
                out.push_str(&format!("{indent}</dict>\n"));
            }
            Plist::Array(values) => {
                out.push_str(&format!("{indent}<array>\n"));
                for value in values {
                    value.write(out, depth + 1);
                }
                out.push_str(&format!("{indent}</array>\n"));
            }
            Plist::String(value) => {
                out.push_str(&format!("{indent}<string>{}</string>\n", escape(value)))
            }
            Plist::Integer(value) => out.push_str(&format!("{indent}<integer>{value}</integer>\n")),
            Plist::Bool(value) => out.push_str(&format!("{indent}<{value}/>\n")),
        }
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parses an XML property list made of the types above. Comments, the XML
/// declaration, and the doctype are skipped; anything else unknown fails.
pub fn parse(xml: &str) -> Option<Plist> {
    let start = xml.find("<plist")?;
    let body = &xml[start..];
    let body = &body[body.find('>')? + 1..];
    let mut parser = Parser { rest: body };
    let value = parser.value()?;
    parser.skip_space();
    parser.rest.starts_with("</plist>").then_some(value)
}

struct Parser<'a> {
    rest: &'a str,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip_space();
        match self.rest.strip_prefix(token) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn text_until(&mut self, close: &str) -> Option<String> {
        let end = self.rest.find(close)?;
        let text = unescape(&self.rest[..end]);
        self.rest = &self.rest[end + close.len()..];
        Some(text)
    }

    fn value(&mut self) -> Option<Plist> {
        if self.eat("<dict/>") {
            return Some(Plist::Dict(Vec::new()));
        }
        if self.eat("<dict>") {
            let mut entries = Vec::new();
            while !self.eat("</dict>") {
                if !self.eat("<key>") {
                    return None;
                }
                let key = self.text_until("</key>")?;
                entries.push((key, self.value()?));
            }
            return Some(Plist::Dict(entries));
        }
        if self.eat("<array/>") {
            return Some(Plist::Array(Vec::new()));
        }
        if self.eat("<array>") {
            let mut values = Vec::new();
            while !self.eat("</array>") {
                values.push(self.value()?);
            }
            return Some(Plist::Array(values));
        }
        if self.eat("<string/>") {
            return Some(Plist::String(String::new()));
        }
        if self.eat("<string>") {
            return self.text_until("</string>").map(Plist::String);
        }
        if self.eat("<integer>") {
            return self
                .text_until("</integer>")?
                .trim()
                .parse()
                .ok()
                .map(Plist::Integer);
        }
        if self.eat("<true/>") {
            return Some(Plist::Bool(true));
        }
        if self.eat("<false/>") {
            return Some(Plist::Bool(false));
        }
        None
    }
}

/// A string from a LaunchAgent's `EnvironmentVariables`. `None` when the
/// file does not parse or does not set it.
pub fn environment_value(xml: &str, key: &str) -> Option<String> {
    parse(xml)?
        .get("EnvironmentVariables")?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_agents_round_trip_their_environment() {
        let plist = Plist::dict(vec![
            ("Label", Plist::String("dev.test".into())),
            (
                "ProgramArguments",
                Plist::strings(["/bin/bash", "-c", "a && b <c>"]),
            ),
            (
                "EnvironmentVariables",
                Plist::dict(vec![
                    ("PORT", Plist::String("4774".into())),
                    ("PROFILE", Plist::String("répo<&>".into())),
                    ("EMPTY", Plist::String(String::new())),
                ]),
            ),
            ("RunAtLoad", Plist::Bool(false)),
            ("ThrottleInterval", Plist::Integer(5)),
        ]);
        let xml = plist.to_xml();
        assert!(xml.contains("<string>a &amp;&amp; b &lt;c&gt;</string>"));
        assert!(xml.contains("<false/>") && xml.contains("<integer>5</integer>"));
        assert_eq!(environment_value(&xml, "PORT").as_deref(), Some("4774"));
        assert_eq!(
            environment_value(&xml, "PROFILE").as_deref(),
            Some("répo<&>")
        );
        assert_eq!(environment_value(&xml, "EMPTY").as_deref(), Some(""));
        assert_eq!(
            environment_value(&xml, "Label"),
            None,
            "only EnvironmentVariables is read"
        );
        assert_eq!(environment_value(&xml, "MISSING"), None);
        assert_eq!(parse(&xml), Some(plist), "what is written parses back");
        assert_eq!(
            parse("<plist version=\"1.0\"><dict><key>a</key><bogus/></dict></plist>"),
            None
        );
    }

    #[test]
    fn plistlib_output_reads_the_same() {
        // Python plistlib's layout, which existing installs carry.
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>GAH_TAILSCALE_SERVE</key>\n\t\t<string>1</string>\n\t</dict>\n</dict>\n</plist>\n";
        assert_eq!(
            environment_value(xml, "GAH_TAILSCALE_SERVE").as_deref(),
            Some("1")
        );
    }
}
