//! A command's arguments: `--name value` options and positional values.

use std::collections::BTreeMap;

/// The arguments after the command's name.
#[derive(Debug, Default)]
pub struct Args {
    options: BTreeMap<String, String>,
    pub positional: Vec<String>,
}

impl Args {
    /// Every `--name` takes a value; anything else is positional. `known`
    /// are the options the command takes.
    pub fn parse(args: impl IntoIterator<Item = String>, known: &[&str]) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let Some(name) = arg.strip_prefix("--") else {
                parsed.positional.push(arg);
                continue;
            };
            if !known.contains(&name) {
                return Err(format!("unknown option --{name}"));
            }
            let value = args.next().ok_or(format!("--{name} needs a value"))?;
            if parsed.options.insert(name.into(), value).is_some() {
                return Err(format!("--{name} is given twice"));
            }
        }
        Ok(parsed)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }

    pub fn required(&self, name: &str) -> Result<&str, String> {
        self.get(name).ok_or(format!("--{name} is required"))
    }

    /// The one positional value, named `what` in errors.
    pub fn one(&self, what: &str) -> Result<&str, String> {
        match self.positional.as_slice() {
            [one] => Ok(one),
            [] => Err(format!("{what} is required")),
            _ => Err(format!("one {what} only")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        Args::parse(args.iter().map(|a| a.to_string()), &["days", "out"])
    }

    #[test]
    fn options_take_a_value_and_the_rest_is_positional() {
        let a = parse(&["--days", "30", "request.csr"]).unwrap();
        assert_eq!(a.get("days"), Some("30"));
        assert_eq!(a.one("a request").unwrap(), "request.csr");
        assert!(a.required("out").unwrap_err().contains("--out"));
        assert!(parse(&["--other", "x"]).unwrap_err().contains("unknown"));
        assert!(parse(&["--days"]).unwrap_err().contains("needs a value"));
        assert!(parse(&["--days", "1", "--days", "2"]).is_err());
        assert!(parse(&["a", "b"]).unwrap().one("a request").is_err());
    }
}
