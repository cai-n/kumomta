use crate::map::CidrMap;
use crate::parse_cidr;
pub use cidr::AnyIpCidr;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct CidrSet(CidrMap<()>);

impl CidrSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn default_trusted_hosts() -> Self {
        vec!["127.0.0.1", "::1"].try_into().unwrap()
    }

    pub fn default_prohibited_hosts() -> Self {
        vec!["127.0.0.0/8", "::1"].try_into().unwrap()
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.0.contains(ip)
    }

    pub fn insert<T: Ord + Into<AnyIpCidr>>(&mut self, value: T) {
        self.0.insert(value.into(), ());
    }
}

impl<T: Ord + Into<AnyIpCidr>, const N: usize> From<[T; N]> for CidrSet {
    /// Converts a `[T; N]` into a `CidrSet`.
    fn from(mut arr: [T; N]) -> Self {
        if N == 0 {
            return CidrSet::new();
        }

        // use stable sort to preserve the insertion order.
        arr.sort();
        let iter = IntoIterator::into_iter(arr); //.map(|k| k.into());
        iter.collect()
    }
}

impl<S> FromIterator<S> for CidrSet
where
    S: Into<AnyIpCidr>,
{
    fn from_iter<I: IntoIterator<Item = S>>(iter: I) -> Self {
        let mut set = CidrMap::new();
        for entry in iter {
            set.insert(entry.into(), ());
        }
        Self(set)
    }
}

impl TryFrom<Vec<&str>> for CidrSet {
    type Error = String;

    fn try_from(v: Vec<&str>) -> Result<Self, String> {
        let mut set = CidrMap::new();
        let mut problems = vec![];
        for entry in v {
            match parse_cidr(&entry) {
                Ok(cidr) => {
                    set.insert(cidr, ());
                }
                Err(err) => {
                    problems.push(format!("{entry}: {err:#}"));
                }
            }
        }
        if problems.is_empty() {
            Ok(Self(set))
        } else {
            Err(problems.join(", "))
        }
    }
}

impl TryFrom<Vec<String>> for CidrSet {
    type Error = String;

    fn try_from(v: Vec<std::string::String>) -> Result<Self, String> {
        let mut set = CidrMap::new();
        let mut problems = vec![];
        for entry in v {
            match parse_cidr(&entry) {
                Ok(cidr) => {
                    set.insert(cidr, ());
                }
                Err(err) => {
                    problems.push(format!("{entry}: {err:#}"));
                }
            }
        }
        if problems.is_empty() {
            Ok(Self(set))
        } else {
            Err(problems.join(", "))
        }
    }
}

impl Into<Vec<String>> for CidrSet {
    fn into(self) -> Vec<String> {
        let mut result = vec![];
        for (key, _unit) in self.0.iter() {
            result.push(key.to_string());
        }
        result
    }
}

impl From<Vec<AnyIpCidr>> for CidrSet {
    fn from(entries: Vec<AnyIpCidr>) -> Self {
        entries.into_iter().collect()
    }
}

impl Into<Vec<AnyIpCidr>> for CidrSet {
    fn into(self) -> Vec<AnyIpCidr> {
        let mut result = vec![];
        for (key, _unit) in self.0.iter() {
            result.push(key.clone());
        }
        result
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn cidrset_any() {
        let empty_set = CidrSet::new();
        let set_with_any: CidrSet = [AnyIpCidr::Any].into();

        assert!(!empty_set.contains("127.0.0.1".parse().unwrap()));
        assert!(set_with_any.contains("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn cidrset() {
        let set: CidrSet = [
            parse_cidr("127.0.0.1").unwrap(),
            parse_cidr("::1").unwrap(),
            parse_cidr("192.168.1.0/24").unwrap(),
            // This entry is overlapped by the preceding entry
            parse_cidr("192.168.1.24").unwrap(),
            parse_cidr("192.168.3.0/28").unwrap(),
            parse_cidr("10.0.3.0/24").unwrap(),
            parse_cidr("10.0.4.0/24").unwrap(),
            parse_cidr("10.0.7.0/24").unwrap(),
        ]
        .into();

        assert!(set.contains("127.0.0.1".parse().unwrap()));
        assert!(!set.contains("127.0.0.2".parse().unwrap()));
        assert!(set.contains("::1".parse().unwrap()));

        assert!(!set.contains("192.168.2.1".parse().unwrap()));

        assert!(set.contains("192.168.1.0".parse().unwrap()));
        assert!(set.contains("192.168.1.1".parse().unwrap()));
        assert!(set.contains("192.168.1.100".parse().unwrap()));
        assert!(set.contains("192.168.1.24".parse().unwrap()));

        assert!(set.contains("192.168.3.0".parse().unwrap()));
        assert!(!set.contains("192.168.3.16".parse().unwrap()));

        // Note that the snapshot does not contain 192.168.1.24/32; that
        // overlaps with the broader 192.168.1.0/24 so is "lost"
        // when extracting the information from the set
        let decompose: Vec<AnyIpCidr> = set.into();
        k9::snapshot!(
            decompose,
            "
[
    V4(
        10.0.3.0/24,
    ),
    V4(
        10.0.4.0/24,
    ),
    V4(
        10.0.7.0/24,
    ),
    V4(
        127.0.0.1/32,
    ),
    V4(
        192.168.1.0/24,
    ),
    V4(
        192.168.3.0/28,
    ),
    V6(
        ::1/128,
    ),
]
"
        );
    }
}
