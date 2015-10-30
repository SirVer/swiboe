use ::{Result, Error};
use regex;

pub struct RpcId {
    name: String,
    implementor: String,
}

static NAME_REGEX: regex::Regex = regex!(r"^[a-z][a-z0-9_.]*[a-z0-9]$");
static IMPLEMETOR_REGEX: regex::Regex = regex!(r"^[a-z][a-z0-9_]*[a-z0-9]$");

pub fn is_valid_name(name: &str) -> bool {
    NAME_REGEX.is_match(name)
}

pub fn is_valid_implementor(name: &str) -> bool {
    IMPLEMETOR_REGEX.is_match(name)
}

impl RpcId {
    pub fn new(identifier: &str) -> Result<RpcId> {
        let splits: Vec<&str> = identifier.splitn(2, ':').collect();
        if splits.len() != 2 {
            return Err(Error::InvalidRpcIdentifier);
        }

        let (name, implementor) = (splits[0], splits[1]);
        if !is_valid_name(name) || !is_valid_implementor(implementor) {
            return Err(Error::InvalidRpcIdentifier);
        }

        let id = RpcId {
            name: name.to_string(),
            implementor: implementor.to_string(),
        };
        Ok(id)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn implementor(&self) -> &str {
        &self.implementor
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_name() {
        assert!(is_valid_name("blub"));
        assert!(is_valid_name("blub.foo.basel"));
        assert!(is_valid_name("bl1b.f3o.basel0"));
        assert!(is_valid_name("new_rpc.f3o.basel0"));

        assert!(!is_valid_name("blUb.foo.basel"));
        assert!(!is_valid_name("blub:foo.basel"));
        assert!(!is_valid_name("blub_"));
        assert!(!is_valid_name("blub."));
        assert!(!is_valid_name(".blub"));
    }

    #[test]
    fn test_identifier_with_string() {
        let id = RpcId::new("blub.blah:swiboe").unwrap();
        assert_eq!("blub.blah", id.name());
        assert_eq!("swiboe", id.implementor());
    }
}
