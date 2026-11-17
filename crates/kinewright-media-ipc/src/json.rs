use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::{collections::BTreeSet, fmt, io};

struct Bounded(u8);
impl<'de> DeserializeSeed<'de> for Bounded {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        if self.0 > 8 {
            return Err(de::Error::custom("JSON depth"));
        }
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Bounded {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate key"));
            }
            map.next_value_seed(Bounded(self.0 + 1))?;
        }
        Ok(())
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
        while seq.next_element_seed(Bounded(self.0 + 1))?.is_some() {}
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
}
pub(super) fn check(bytes: &[u8]) -> io::Result<()> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    Bounded(0)
        .deserialize(&mut decoder)
        .and_then(|()| decoder.end())
        .map_err(|_| super::invalid())
}
