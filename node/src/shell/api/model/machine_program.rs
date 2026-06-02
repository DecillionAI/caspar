//! Translation of `shell/api/model/machine_program.go`.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::transaction::ITrx;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Machine {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "chainId", default)]
    pub chain_id: String,
    #[serde(rename = "shardChainId", default)]
    pub shard_chain_id: String,
    #[serde(rename = "ownerId", default)]
    pub owner_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(rename = "machinesCount", default)]
    pub machines_count: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub desc: String,
}

impl Machine {
    pub fn type_() -> &'static str {
        "Machine"
    }

    pub fn push(&self, trx: &dyn ITrx) {
        let mut cols: HashMap<String, Vec<u8>> = HashMap::new();
        cols.insert("id".into(), self.id.as_bytes().to_vec());
        cols.insert("ownerId".into(), self.owner_id.as_bytes().to_vec());
        cols.insert("username".into(), self.username.as_bytes().to_vec());
        cols.insert("chainId".into(), self.chain_id.as_bytes().to_vec());
        cols.insert(
            "shardChainId".into(),
            self.shard_chain_id.as_bytes().to_vec(),
        );
        cols.insert(
            "machinesCount".into(),
            (self.machines_count as u32).to_le_bytes().to_vec(),
        );
        trx.put_obj(Self::type_(), &self.id, cols);
    }

    fn fill(d: &mut Machine, m: &HashMap<String, Vec<u8>>) {
        if let Some(v) = m.get("id") {
            d.id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("ownerId") {
            d.owner_id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("username") {
            d.username = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("chainId") {
            d.chain_id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("shardChainId") {
            d.shard_chain_id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("machinesCount") {
            if v.len() == 4 {
                d.machines_count =
                    i64::from(u32::from_le_bytes(v.as_slice().try_into().unwrap()));
            }
        }
    }

    pub fn pull(mut self, trx: &dyn ITrx, with_profile: bool) -> Machine {
        let m = trx.get_obj(Self::type_(), &self.id);
        if !m.is_empty() {
            Machine::fill(&mut self, &m);
            if with_profile {
                if let Ok(meta) = trx.get_json(&format!("MachineMeta::{}", self.id), "metadata.public.profile") {
                    if let Some(s) = meta.get("title").and_then(|v| v.as_str()) {
                        self.title = s.to_string();
                    }
                    if let Some(s) = meta.get("avatar").and_then(|v| v.as_str()) {
                        self.avatar = s.to_string();
                    }
                    if let Some(s) = meta.get("desc").and_then(|v| v.as_str()) {
                        self.desc = s.to_string();
                    }
                }
            }
        }
        self
    }

    pub fn delete(&self, trx: &dyn ITrx) {
        for c in [
            "|",
            "id",
            "username",
            "chainId",
            "shardChainId",
            "machinesCount",
            "ownerId",
        ] {
            trx.del_key(&format!("obj::{}::{}::{}", Self::type_(), self.id, c));
        }
        trx.del_json(&format!("MachineMeta::{}", self.id), "metadata");
    }

    pub fn list(trx: &dyn ITrx, prefix: &str) -> Result<Vec<Machine>> {
        let mut list = trx.get_links_list(prefix, -1, -1, &[])?;
        for entry in list.iter_mut() {
            if let Some(stripped) = entry.strip_prefix(prefix) {
                *entry = stripped.to_string();
            }
        }
        let objs = trx.get_obj_list("Machine", &list, &HashMap::new(), &[])?;
        let mut entities: Vec<Machine> = objs
            .into_iter()
            .filter_map(|(id, m)| {
                if m.is_empty() {
                    return None;
                }
                let mut d = Machine {
                    id,
                    ..Default::default()
                };
                Machine::fill(&mut d, &m);
                Some(d)
            })
            .collect();
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }

    pub fn all(trx: &dyn ITrx, offset: i64, count: i64) -> Result<Vec<Machine>> {
        let objs = trx.get_obj_list(
            "Machine",
            &["*".to_string()],
            &HashMap::new(),
            &[offset, count],
        )?;
        let mut entities: Vec<Machine> = objs
            .into_iter()
            .filter_map(|(id, m)| {
                if m.is_empty() {
                    return None;
                }
                let mut d = Machine {
                    id,
                    ..Default::default()
                };
                Machine::fill(&mut d, &m);
                Some(d)
            })
            .collect();
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Program {
    // The program's own identity.
    #[serde(rename = "id", default)]
    pub id: String,
    // Link to the owning Machine object (machines = former "apps"). This is the
    // canonical owner relationship; there is no separate app_id pointer.
    #[serde(rename = "machineId", default)]
    pub machine_id: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub comment: String,
}

impl Program {
    pub fn type_() -> &'static str {
        "Program"
    }

    pub fn push(&self, trx: &dyn ITrx) {
        let mut cols: HashMap<String, Vec<u8>> = HashMap::new();
        // Persisted column "id" holds the program's own id; "machineId" holds
        // the owning machine id.
        cols.insert("id".into(), self.id.as_bytes().to_vec());
        cols.insert("machineId".into(), self.machine_id.as_bytes().to_vec());
        cols.insert("runtime".into(), self.runtime.as_bytes().to_vec());
        cols.insert("path".into(), self.path.as_bytes().to_vec());
        cols.insert("comment".into(), self.comment.as_bytes().to_vec());
        trx.put_obj(Self::type_(), &self.id, cols);
    }

    fn fill(d: &mut Program, m: &HashMap<String, Vec<u8>>) {
        if let Some(v) = m.get("id") {
            d.id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("machineId") {
            d.machine_id = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("runtime") {
            d.runtime = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("path") {
            d.path = String::from_utf8_lossy(v).into_owned();
        }
        if let Some(v) = m.get("comment") {
            d.comment = String::from_utf8_lossy(v).into_owned();
        }
    }

    pub fn pull(mut self, trx: &dyn ITrx) -> Program {
        let m = trx.get_obj(Self::type_(), &self.id);
        if !m.is_empty() {
            Program::fill(&mut self, &m);
        }
        self
    }

    pub fn all(trx: &dyn ITrx, offset: i64, count: i64) -> Result<Vec<Program>> {
        let objs = if count == -1 {
            trx.get_obj_list("Program", &["*".to_string()], &HashMap::new(), &[])?
        } else {
            trx.get_obj_list(
                "Program",
                &["*".to_string()],
                &HashMap::new(),
                &[offset, count],
            )?
        };
        let mut entities: Vec<Program> = objs
            .into_iter()
            .filter_map(|(id, m)| {
                if m.is_empty() {
                    return None;
                }
                let mut d = Program {
                    id,
                    ..Default::default()
                };
                Program::fill(&mut d, &m);
                Some(d)
            })
            .collect();
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }

    pub fn list(trx: &dyn ITrx, prefix: &str) -> Result<Vec<Program>> {
        let mut list = trx.get_links_list(prefix, -1, -1, &[])?;
        for entry in list.iter_mut() {
            if let Some(stripped) = entry.strip_prefix(prefix) {
                *entry = stripped.to_string();
            }
        }
        let objs = trx.get_obj_list("Program", &list, &HashMap::new(), &[])?;
        let mut entities: Vec<Program> = objs
            .into_iter()
            .filter_map(|(id, m)| {
                if m.is_empty() {
                    return None;
                }
                let mut d = Program {
                    id,
                    ..Default::default()
                };
                Program::fill(&mut d, &m);
                Some(d)
            })
            .collect();
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }
}
