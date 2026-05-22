//! Translation of `shell/api/model/creature.go`.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::abstractions::models::trx::ITrx;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Creature {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub type_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(rename = "publicKey", default, skip_serializing_if = "String::is_empty")]
    pub public_key: String,
    #[serde(rename = "chainId", default)]
    pub chain_id: String,
    #[serde(rename = "subchainId", default)]
    pub subchain_id: String,
    #[serde(rename = "ownerId", default, skip_serializing_if = "String::is_empty")]
    pub owner_id: String,
    #[serde(
        rename = "machinesCount",
        default,
        skip_serializing_if = "i64_is_zero"
    )]
    pub machines_count: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub avatar: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub desc: String,
    #[serde(default)]
    pub balance: i64,
}

fn i64_is_zero(n: &i64) -> bool {
    *n == 0
}

impl Creature {
    pub fn type_() -> &'static str {
        "Creature"
    }

    pub fn push(&self, trx: &dyn ITrx) {
        let bal = (self.balance as u64).to_le_bytes().to_vec();
        let mut cols: HashMap<String, Vec<u8>> = HashMap::new();
        cols.insert("type".into(), self.type_name.as_bytes().to_vec());
        cols.insert("username".into(), self.username.as_bytes().to_vec());
        cols.insert("publicKey".into(), self.public_key.as_bytes().to_vec());
        cols.insert("chainId".into(), self.chain_id.as_bytes().to_vec());
        cols.insert("subchainId".into(), self.subchain_id.as_bytes().to_vec());
        cols.insert("ownerId".into(), self.owner_id.as_bytes().to_vec());
        cols.insert("balance".into(), bal);
        trx.put_obj(Self::type_(), &self.id, cols);
        if !self.username.is_empty() {
            trx.put_index(
                "Creature",
                "username",
                "id",
                &self.username,
                self.id.as_bytes().to_vec(),
            );
        }
    }

    fn fill(d: &mut Creature, m: &HashMap<String, Vec<u8>>) {
        if let Some(v) = m.get("type") {
            d.type_name = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("username") {
            d.username = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("publicKey") {
            d.public_key = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("chainId") {
            d.chain_id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("subchainId") {
            d.subchain_id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("ownerId") {
            d.owner_id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("balance") {
            if v.len() == 8 {
                d.balance = i64::from_le_bytes(v.as_slice().try_into().unwrap());
            }
        }
    }

    pub fn pull(mut self, trx: &dyn ITrx) -> Creature {
        let m = trx.get_obj(Self::type_(), &self.id);
        if !m.is_empty() {
            Creature::fill(&mut self, &m);
        }
        self
    }

    pub fn all(trx: &dyn ITrx, offset: i64, count: i64) -> Result<Vec<Creature>> {
        let objs = trx.get_obj_list(
            "Creature",
            &["*".to_string()],
            &HashMap::new(),
            &[offset, count],
        )?;
        let mut entities: Vec<Creature> = objs
            .into_iter()
            .filter_map(|(id, m)| {
                if m.is_empty() {
                    return None;
                }
                let mut c = Creature {
                    id,
                    ..Default::default()
                };
                Creature::fill(&mut c, &m);
                Some(c)
            })
            .collect();
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }
}
