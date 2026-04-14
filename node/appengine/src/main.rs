use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use blockingqueue::BlockingQueue;
use bollard::container::{
    Config as DockerConfig, CreateContainerOptions, LogOutput, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions, UploadToContainerOptions,
};
use bollard::errors::Error as BollardError;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::image::BuildImageOptions;
use bollard::models::HostConfig;
use bollard::Docker;
use elpian_vm::api as elpian_api;
use elpify_lang::{
    execute_masm_file_with_proof, stack_outputs_from_ints, transpile_js_to_masm, verify_execution,
    ExecutionEngine, TaskInput,
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
use std::process::{Child, Command};
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

include!("globals.rs");
include!("models/base_vm.rs");
include!("models/docker_vm.rs");
include!("models/fire_vm.rs");
include!("models/wasm_vm.rs");
include!("models/elpian_vm.rs");
include!("models/elpify_vm.rs");
include!("models/javascript_vm.rs");
include!("models/runtime_models.rs");
include!("models/vm_runtime.rs");
include!("controllers/vm_controller.rs");
include!("controllers/docker_vm_controller.rs");
include!("controllers/fire_vm_controller.rs");
include!("controllers/wasm_vm_controller.rs");
include!("controllers/elpian_vm_controller.rs");
include!("controllers/elpify_vm_controller.rs");
include!("controllers/javascript_vm_controller.rs");
include!("network/vm_network.rs");
include!("network/gateway_types.rs");
include!("network/gateway_registry.rs");
include!("network/gateway_http.rs");
include!("network/gateway_socket.rs");
include!("network/gateway.rs");
include!("host/vm_host_functions.rs");
include!("host/task_graph.rs");
include!("bridge/messaging.rs");
include!("bridge/zmq_packet_types.rs");
include!("bridge/zmq_bridge_api.rs");
include!("bridge/zmq_packet_dispatcher.rs");
include!("bootstrap/restore.rs");
include!("bootstrap/engine.rs");
