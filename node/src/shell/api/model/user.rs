//! Translation of `shell/api/model/user.go`.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::transaction::ITrx;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct User {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub typ: String,
    #[serde(default)]
    pub username: String,
    #[serde(rename = "publicKey", default)]
    pub public_key: String,
    #[serde(default)]
    pub balance: i64,
}

impl User {
    pub fn type_() -> &'static str {
        "User"
    }

    fn balance_bytes(&self) -> Vec<u8> {
        (self.balance as u64).to_le_bytes().to_vec()
    }

    pub fn push(&self, trx: &dyn ITrx) {
        let mut cols: HashMap<String, Vec<u8>> = HashMap::new();
        cols.insert("type".into(), self.typ.as_bytes().to_vec());
        cols.insert("username".into(), self.username.as_bytes().to_vec());
        cols.insert("publicKey".into(), self.public_key.as_bytes().to_vec());
        cols.insert("balance".into(), self.balance_bytes());
        trx.put_obj(Self::type_(), &self.id, cols);
        trx.put_index(
            "User",
            "username",
            "id",
            &self.username,
            self.id.as_bytes().to_vec(),
        );
    }

    pub fn delete(&self, trx: &dyn ITrx) {
        for c in ["|", "id", "type", "username", "publicKey", "balance"] {
            trx.del_key(&format!("obj::{}::{}::{}", Self::type_(), self.id, c));
        }
        trx.del_json(&format!("UserMeta::{}", self.id), "metadata");
    }

    pub fn pull(mut self, trx: &dyn ITrx) -> User {
        let m = trx.get_obj(Self::type_(), &self.id);
        if !m.is_empty() {
            User::fill(&mut self, &m);
        }
        self
    }

    fn fill(d: &mut User, m: &HashMap<String, Vec<u8>>) {
        if let Some(v) = m.get("type") {
            d.typ = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("username") {
            d.username = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("publicKey") {
            d.public_key = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("balance") {
            if v.len() == 8 {
                d.balance = i64::from_le_bytes(v.as_slice().try_into().unwrap());
            }
        }
    }

    pub fn list(
        trx: &dyn ITrx,
        prefix: &str,
        query: &HashMap<String, String>,
    ) -> Result<Vec<User>> {
        let mut list = trx.get_links_list(prefix, -1, -1, &[])?;
        for entry in list.iter_mut() {
            if let Some(stripped) = entry.strip_prefix(prefix) {
                *entry = stripped.to_string();
            }
        }
        let objs = trx.get_obj_list("User", &list, query, &[])?;
        let mut entities: Vec<User> = objs
            .into_iter()
            .filter_map(|(id, m)| {
                if m.is_empty() {
                    return None;
                }
                let mut u = User {
                    id,
                    ..Default::default()
                };
                User::fill(&mut u, &m);
                Some(u)
            })
            .collect();
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }

    pub fn all(
        trx: &dyn ITrx,
        offset: i64,
        count: i64,
        query: &HashMap<String, String>,
    ) -> Result<Vec<User>> {
        let objs = trx.get_obj_list("User", &["*".to_string()], query, &[offset, count])?;
        let mut entities: Vec<User> = objs
            .into_iter()
            .filter_map(|(id, m)| {
                if m.is_empty() {
                    return None;
                }
                let mut u = User {
                    id,
                    ..Default::default()
                };
                User::fill(&mut u, &m);
                Some(u)
            })
            .collect();
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }

    pub fn search(
        trx: &dyn ITrx,
        offset: i64,
        count: i64,
        from_column: &str,
        word: &str,
        filter: &HashMap<String, String>,
    ) -> Result<Vec<User>> {
        let links = trx.search_link_vals_list("User", from_column, "id", word, filter, offset, count)?;
        let objs = trx.get_obj_list("User", &links, &HashMap::new(), &[])?;
        let mut entities: Vec<User> = objs
            .into_iter()
            .filter_map(|(id, m)| {
                if m.is_empty() {
                    return None;
                }
                let mut u = User {
                    id,
                    ..Default::default()
                };
                User::fill(&mut u, &m);
                Some(u)
            })
            .collect();
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }
}
