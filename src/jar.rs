//! A cookie jar implementation for storing a set of cookies.
//!
//! This CookieJar type can be used to manage a session of cookies by keeping
//! track of cookies that are added and deleted over time. It provides a method,
//! `delta`, which will calculate the number of `Set-Cookie` headers that need
//! to be sent back to a client which tracks the changes in the lifetime of the
//! jar itself.
//!
//! A cookie jar can also be borrowed to a child cookie jar with new
//! functionality such as automatically signing cookies, storing permanent
//! cookies, etc. This functionality can also be chaned together.

use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entries;

use time;

use Cookie;

/// A jar of cookies for managing a session
///
/// # Example
///
/// ```
/// # extern crate cookie;
/// # fn main() {
/// use cookie::{Cookie, CookieJar};
///
/// let mut c = CookieJar::new(b"f8f9eaf1ecdedff5e5b749c58115441e");
///
/// // Add a cookie to this jar
/// c.add(Cookie::new("key".to_string(), "value".to_string()));
///
/// // Remove the added cookie
/// c.remove("test");
///
/// // Add a signed cookie to the jar
/// c.signed().add(Cookie::new("key".to_string(), "value".to_string()));
///
/// // Add a signed and encrypted cookie to the jar
/// c.encrypted().add(Cookie::new("key".to_string(), "value".to_string()));
///
/// // Add a permanently signed cookie to the jar
/// c.permanent().signed()
///  .add(Cookie::new("key".to_string(), "value".to_string()));
///
/// // Add a permanently signed and encrypted cookie to the jar
/// c.permanent().encrypted()
///  .add(Cookie::new("key".to_string(), "value".to_string()));
/// # }
/// ```
pub struct CookieJar<'a> {
    flavor: Flavor<'a>,
}

enum Flavor<'a> {
    Child(Child<'a>),
    Root(Root),
}

struct Child<'a> {
    parent: &'a mut CookieJar<'a>,
    read: Read,
    write: Write,
}

type Read = fn(&Root, Cookie) -> Option<Cookie>;
type Write = fn(&Root, Cookie) -> Cookie;

struct Root {
    map: HashMap<String, Cookie>,
    new_cookies: HashSet<String>,
    removed_cookies: HashSet<String>,
    key: Vec<u8>,
}

impl<'a> CookieJar<'a> {
    /// Creates a new empty cookie jar with the given signing key.
    ///
    /// The given key is used to sign cookies in the signed cookie jar.
    pub fn new(key: &[u8]) -> CookieJar<'static> {

        let normalized_key = if key.len() >= secure::MIN_KEY_LEN {
            key.to_vec()
        } else {
            // Using a SHA-256 hash to normalize key as Rails suggests.
            // See https://github.com/rails/rails/blob/master/activesupport/lib/active_support/message_encryptor.rb
            secure::prepare_key(key)
        };

        CookieJar {
            flavor: Flavor::Root(Root {
                map: HashMap::new(),
                new_cookies: HashSet::new(),
                removed_cookies: HashSet::new(),
                key: normalized_key,
            })
        }
    }

    /// Adds an original cookie from a request.
    ///
    /// This method only works on the root cookie jar and is not intended for
    /// use during the lifetime of a request, it is intended to initialize a
    /// cookie jar from an incoming request.
    pub fn add_original(&mut self, cookie: Cookie) {
        match self.flavor {
            Flavor::Child(..) => panic!("can't add an original cookie to a child jar!"),
            Flavor::Root(ref mut root) => {
                let name = cookie.name.clone();
                root.map.insert(name, cookie);
            }
        }
    }

    fn root<'a>(&'a self) -> &'a Root {
        match self.flavor {
            Flavor::Child(ref child) => child.parent.root(),
            Flavor::Root(ref me) => return me
        }
    }

    fn root_mut<'a>(&'a mut self) -> &'a mut Root {
        match self.flavor {
            Flavor::Child(ref mut child) => child.parent.root_mut(),
            Flavor::Root(ref mut me) => return me
        }
    }

    /// Adds a new cookie to this cookie jar.
    ///
    /// If this jar is a child cookie jar, this will walk up the chain of
    /// borrowed jars, modifying the cookie as it goes along.
    pub fn add(&mut self, cookie: Cookie) {
        let cookie = self.write(self.root(), cookie);
        let root = self.root_mut();

        let name = cookie.name.clone();
        root.map.insert(name.clone(), cookie);
        root.removed_cookies.remove(&name);
        root.new_cookies.insert(name);
    }

    /// Removes a cookie from this cookie jar.
    pub fn remove(&mut self, cookie: &str) {
        let cookie = cookie.to_string();
        let root = self.root_mut();

        root.map.remove(&cookie);
        root.new_cookies.remove(&cookie);
        root.removed_cookies.insert(cookie);
    }

    /// Finds a cookie inside of this cookie jar.
    ///
    /// The cookie is subject to modification by any of the child cookie jars
    /// that are currently borrowed. A copy of the cookie is returned.
    pub fn find(&self, name: &str) -> Option<Cookie> {
        let root = self.root();
        let name = name.to_string();
        if root.removed_cookies.contains(&name) {
            return None
        }
        root.map.get(&name).and_then(|c| self.try_read(root, Some(c.clone())))
    }

    /// Creates a child signed cookie jar.
    ///
    /// All cookies read from the child jar will require a valid signature and
    /// all cookies written will be signed automatically.
    pub fn signed(&'a mut self) -> CookieJar<'a> {
        CookieJar {
            flavor: Flavor::Child(Child {
                parent: self,
                read: secure::design,
                write: secure::sign,
            })
        }
    }

    /// Creates a child encrypted cookie jar.
    ///
    /// All cookies read from the child jar must be encrypted and signed by a valid key and
    /// all cookies written will be encrypted and signed automatically.
    pub fn encrypted(&'a mut self) -> CookieJar<'a> {
        return CookieJar {
            flavor: Flavor::Child(Child {
                parent: self,
                read: secure::design_and_decrypt,
                write: secure::encrypt_and_sign,
            })
        };
    }

    /// Creates a child jar for permanent cookie storage.
    ///
    /// All cookies written to the child jar will have an expiration date 20
    /// years into the future to ensure they stick around for a long time.
    pub fn permanent(&'a mut self) -> CookieJar<'a> {
        return CookieJar {
            flavor: Flavor::Child(Child {
                parent: self,
                read: read,
                write: write,
            })
        };

        fn read(_root: &Root, cookie: Cookie) -> Option<Cookie> {
            Some(cookie)
        }

        fn write(_root: &Root, mut cookie: Cookie) -> Cookie {
            // Expire 20 years in the future
            cookie.max_age = Some(3600 * 24 * 365 * 20);
            let mut now = time::now();
            now.tm_year += 20;
            cookie.expires = Some(now);
            cookie
        }
    }

    /// Calculates the changes that have occurred to this cookie jar over time,
    /// returning a vector of `Set-Cookie` headers.
    pub fn delta(&self) -> Vec<Cookie> {
        let root = self.root();
        let mut ret = Vec::new();
        for cookie in root.removed_cookies.iter() {
            let mut c = Cookie::new(cookie.clone(), String::new());
            c.max_age = Some(0);
            let mut now = time::now();
            now.tm_year -= 1;
            c.expires = Some(now);
            ret.push(c);
        }
        let map = &root.map;
        for cookie in root.new_cookies.iter() {
            ret.push(map.get(cookie).unwrap().clone());
        }
        return ret;
    }

    pub fn iter_all<'a>(&'a self) -> Entries<'a, String, Cookie> {
        self.root().map.iter()
    }

    pub fn iter(&'a self) -> Cookies<'a> {
        Cookies::new(self)
    }

    fn write(&self, root: &Root, mut cookie: Cookie) -> Cookie {
        match self.flavor {
            Flavor::Child(ref child) => {
                cookie = (child.write)(root, cookie);
                child.parent.write(root, cookie)
            }
            Flavor::Root(..) => cookie
        }
    }

    fn try_read(&self, root: &Root, cookie: Option<Cookie>) -> Option<Cookie> {
        match self.flavor {
            _ if cookie.is_none() => return None,
            Flavor::Child(ref child) => {
                child.parent.try_read(root, (child.read)(root, cookie.unwrap()))
            }
            Flavor::Root(..) => cookie,
        }
    }
}

pub struct Cookies<'a> {
    jar: &'a CookieJar<'a>,
    iter: Entries<'a, String, Cookie>,
    root: &'a Root
}

impl<'a> Cookies<'a> {
    pub fn new<'a>(jar: &'a CookieJar<'a>) -> Cookies<'a> {
        Cookies {
            jar: jar,
            iter: jar.iter_all(),
            root: jar.root()
        }
    }
}

impl<'a> Iterator<Cookie> for Cookies<'a> {
    fn next(&mut self) -> Option<Cookie> {
        loop {
            match self.iter.next() {
                Some((_, cookie)) => {
                    match self.jar.try_read(self.root, Some(cookie.clone())) {
                        Some(result_cookie) => return Some(result_cookie),
                        None => continue
                    }
                },
                None => return None
            }
        }
    }

    fn size_hint(&self) -> (uint, Option<uint>) {
        return self.iter.size_hint()
    }
}

mod secure {
    use jar::{Root};
    use {Cookie};
    use openssl::crypto::{hmac, hash, memcmp, symm};
    use serialize::hex::{ToHex, FromHex};

    pub const MIN_KEY_LEN: uint = 32;

    // If a SHA1 HMAC is good enough for rails, it's probably good enough
    // for us as well:
    //
    // https://github.com/rails/rails/blob/master/activesupport/lib
    //                   /active_support/message_verifier.rb#L70
    pub fn sign(root: &Root, mut cookie: Cookie) -> Cookie {
        let signature = dosign(root, cookie.value.as_slice());
        cookie.value.push_str("--");
        cookie.value.push_str(signature.as_slice().to_hex().as_slice());
        cookie
    }

    fn split_value(val: &str) -> Option<(&str, Vec<u8>)> {
        let parts = val.split_str("--");
        let ext = match parts.last() {
            Some(ext) => ext,
            _ => return None,
        };
        let val_len = val.len();
        if ext.len() == val_len { return None }
        let text = val.slice_to(val_len - ext.len() - 2);
        let ext = match ext.from_hex() {
            Ok(sig) => sig, Err(..) => return None,
        };

        Some((text, ext))
    }

    pub fn design(root: &Root, mut cookie: Cookie) -> Option<Cookie> {
        let len = {
            let (text, signature) = match split_value(cookie.value.as_slice()) {
                Some(pair) => pair, None => return None
            };
            let expected = dosign(root, text);
            if !memcmp::eq(expected.as_slice(), signature.as_slice()) {
                return None
            }
            text.len()
        };
        cookie.value.truncate(len);
        Some(cookie)
    }

    fn dosign(root: &Root, val: &str) -> Vec<u8> {
        let mut hmac = hmac::HMAC(hash::SHA1, root.key.as_slice());
        hmac.update(val.as_bytes());
        hmac.finalize()
    }

    // Implementation details were taken from Rails. See
    // https://github.com/rails/rails/blob/master/activesupport/lib/active_support/message_encryptor.rb#L57
    pub fn encrypt_and_sign(root: &Root, mut cookie: Cookie) -> Cookie {
        let encrypted_data = encrypt_data(root, cookie.value.as_slice());
        cookie.value = encrypted_data;
        sign(root, cookie)
    }

    fn encrypt_data(root: &Root, val: &str) -> String {
        let iv = random_iv();
        let iv_str = iv.as_slice().to_hex();

        let mut encrypted_data = symm::encrypt(
            symm::AES_256_CBC,
            root.key.as_slice().slice_to(MIN_KEY_LEN),
            iv,
            val.as_bytes()
        ).as_slice().to_hex();

        encrypted_data.push_str("--");
        encrypted_data.push_str(iv_str.as_slice());
        encrypted_data
    }

    pub fn design_and_decrypt(root: &Root, cookie: Cookie) -> Option<Cookie> {
        let mut cookie = match design(root, cookie) {
            Some(cookie) => cookie,
            None => return None
        };

        let decrypted_data = decrypt_data(root, cookie.value.as_slice()).and_then(|data| String::from_utf8(data).ok());
        match decrypted_data {
            Some(val) => { cookie.value = val; Some(cookie) },
            None => return None
        }
    }

    fn decrypt_data(root: &Root, val: &str) -> Option<Vec<u8>> {
        let (val, iv) = match split_value(val) {
            Some(pair) => pair, None => return None
        };

        let actual = match val.as_slice().from_hex() {
            Ok(actual) => actual, Err(_) => return None
        };

        Some(symm::decrypt(
            symm::AES_256_CBC,
            root.key.as_slice().slice_to(MIN_KEY_LEN),
            iv,
            actual.as_slice()
        ))
    }

    fn random_iv() -> Vec<u8> {
        ::openssl::crypto::rand::rand_bytes(16)
    }

    pub fn prepare_key(key: &[u8]) -> Vec<u8> {
        hash::hash(hash::SHA256, key)
    }
}

#[cfg(test)]
mod test {
    use {Cookie, CookieJar};

    const KEY: &'static [u8] = b"f8f9eaf1ecdedff5e5b749c58115441e";

    #[test]
    fn short_key() {
        CookieJar::new(b"foo");
    }

    #[test]
    fn simple() {
        let mut c = CookieJar::new(KEY);

        c.add(Cookie::new("test".to_string(), "".to_string()));
        c.add(Cookie::new("test2".to_string(), "".to_string()));
        c.remove("test");

        assert!(c.find("test").is_none());
        assert!(c.find("test2").is_some());
    }

    macro_rules! secure_behaviour(
        ($c:ident, $secure:ident) => ({
            $c.$secure().add(Cookie::new("test".to_string(), "test".to_string()));
            assert!($c.find("test").unwrap().value.as_slice() != "test");
            assert!($c.$secure().find("test").unwrap().value.as_slice() == "test");

            let mut cookie = $c.find("test").unwrap();
            cookie.value.push('l');
            $c.add(cookie);
            assert!($c.$secure().find("test").is_none());

            let mut cookie = $c.find("test").unwrap();
            cookie.value = "foobar".to_string();
            $c.add(cookie);
            assert!($c.$secure().find("test").is_none());
        })
    )

    #[test]
    fn signed() {
        let mut c = CookieJar::new(KEY);
        secure_behaviour!(c, signed)
    }

    #[test]
    fn encrypted() {
        let mut c = CookieJar::new(KEY);
        secure_behaviour!(c, encrypted)
    }

    #[test]
    fn permanent() {
        let mut c = CookieJar::new(KEY);

        c.permanent().add(Cookie::new("test".to_string(), "test".to_string()));

        let cookie = c.find("test").unwrap();
        assert_eq!(cookie.value.as_slice(), "test");
        assert_eq!(c.permanent().find("test").unwrap().value.as_slice(), "test");
        assert!(cookie.expires.is_some());
        assert!(cookie.max_age.is_some());
    }

    #[test]
    fn chained() {
        let mut c = CookieJar::new(KEY);

        c.permanent().signed()
         .add(Cookie::new("test".to_string(), "test".to_string()));

        let cookie = c.signed().find("test").unwrap();
        assert_eq!(cookie.value.as_slice(), "test");
        assert!(cookie.expires.is_some());
        assert!(cookie.max_age.is_some());
    }

    #[test]
    fn iter() {
        let mut c = CookieJar::new(KEY);

        c.add_original(Cookie::new("original".to_string(), "original".to_string()));

        c.add(Cookie::new("test".to_string(), "test".to_string()));
        c.add(Cookie::new("test2".to_string(), "test2".to_string()));
        c.add(Cookie::new("test3".to_string(), "test3".to_string()));
        c.add(Cookie::new("test4".to_string(), "test4".to_string()));

        c.signed()
         .add(Cookie::new("signed".to_string(), "signed".to_string()));
        
        c.encrypted()
         .add(Cookie::new("encrypted".to_string(), "encrypted".to_string()));

        c.remove("test");

        let cookies: Vec<Cookie> = c.iter().collect();
        assert_eq!(cookies.len(), 6);

        let encrypted_cookies: Vec<Cookie> = c.encrypted().iter().collect();
        assert_eq!(encrypted_cookies.len(), 1);

        let signged_cookies: Vec<Cookie> = c.signed().iter().collect();
        assert_eq!(signged_cookies.len(), 2);
    }
}
