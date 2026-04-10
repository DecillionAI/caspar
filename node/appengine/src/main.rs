use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use blockingqueue::BlockingQueue;
use bollard::container::{
    Config as DockerConfig, CreateContainerOptions, LogOutput, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions, UploadToContainerOptions,
};
use bollard::errors::Error as BollardError;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::image::BuildImageOptions;
use bollard::models::HostConfig;
use bollard::Docker;
use elpian_vm::api as elpian_api;
use elpify_lang::{
    execute_masm_file_with_proof, stack_outputs_from_ints, verify_execution, ExecutionEngine,
    TaskInput,
};
use futures_util::stream::TryStreamExt;
use once_cell::sync::Lazy;
use reqwest::blocking::Client;
use reqwest::Method;
use rocksdb::{
    Options, ReadOptions, TransactionDB, TransactionDBOptions, TransactionOptions, WriteOptions,
};
use serde_json::{json, Value as JsonValue};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Cursor;
use std::ops::DerefMut;
use std::path::Path;
use std::process::Child;
use std::str;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tar::Builder as TarBuilder;
use timedmap::TimedMap;
use wasmedge_sys::{
    config::Config, AsInstance, CallingFrame, Executor, Function, ImportModule, Instance, Loader,
    Statistics, Store, Validator, WasmValue,
};
use wasmedge_types::{error::CoreError, ValType};

include!("appengine_refactor/globals.rs");
include!("appengine_refactor/docker_controller.rs");
include!("appengine_refactor/fire_controller.rs");
include!("appengine_refactor/host_ops.rs");
include!("appengine_refactor/bootstrap.rs");
include!("appengine_refactor/runtime/messaging.rs");
include!("appengine_refactor/runtime/models.rs");
include!("appengine_refactor/runtime/vm_runtime.rs");
include!("appengine_refactor/runtime/task_graph.rs");
