//! The session's account, from `/etc/passwd`.

/// The user and group ids of the account called `name`.
pub fn ids(name: &str) -> Option<(u32, u32)> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    ids_in(&passwd, name)
}

fn ids_in(passwd: &str, name: &str) -> Option<(u32, u32)> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        let _password = fields.next()?;
        Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_s_ids_are_its_line_s() {
        let passwd = "root:x:0:0:root:/root:/bin/sh\ndisplay:x:100:100::/home/display:/bin/false\n";
        assert_eq!(ids_in(passwd, "display"), Some((100, 100)));
        assert_eq!(ids_in(passwd, "root"), Some((0, 0)));
        assert_eq!(ids_in(passwd, "nobody"), None);
        assert_eq!(ids_in("display:x:abc:1::/:/x\n", "display"), None);
    }
}
