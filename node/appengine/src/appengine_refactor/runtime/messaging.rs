fn wasm_send(mut data: JsonValue) -> std::string::String {
    let req_id = REQ_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    data["requestId"] = JsonValue::from(req_id);
    let cv_ = Arc::new(Condvar::new());
    {
        let cv_clone = Arc::clone(&cv_);
        let mut tgm_lock = TRIGGER_MAP.lock().unwrap();
        tgm_lock.insert(req_id, cv_clone, Duration::from_secs(180));
    }
    {
        GLOBAL_REQ_CHAN.clone().push(data.to_string());
    }
    let responses = Arc::clone(&RESP_MAP);
    let res = {
        {
            let responses_ref = responses.lock().unwrap();
            let _mg: std::sync::MutexGuard<'_, TimedMap<i64, String>> = cv_
                .wait_while(responses_ref, |res| !res.contains(&req_id))
                .unwrap();
        }
        let responses_lock: std::sync::MutexGuard<'_, TimedMap<i64, String>> =
            responses.lock().unwrap();
        let r = responses_lock.remove(&req_id);
        let r_final = if r.is_none() {
            "".to_string()
        } else {
            r.unwrap().clone()
        };
        r_final
    };
    let mut tgm_lock = TRIGGER_MAP.lock().unwrap();
    tgm_lock.remove(&req_id);
    res.to_string()
}

fn log(text: String) {
    let j = json!({
        "key": "log",
        "input": {
            "text": text
        }
    });
    wasm_send(j);
}

