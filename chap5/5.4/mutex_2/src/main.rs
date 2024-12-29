use std::{sync::Arc, time};

use tokio::sync::Mutex;

const NUM_TASKS: usize = 8;

/// ロック中に await するなら、tokio の Mutex を使う必要がある

// ロックだけするタスク
async fn lock_only(v: Arc<Mutex<u64>>) {
    println!("--- begin lock_only");
    let mut n = v.lock().await;
    *n += 1;
    println!("--- end lock_only");
}

// ロック中に await を行うタスク
async fn lock_sleep(v: Arc<Mutex<u64>>) {
    println!("--- begin lock_sleep");
    let mut n = v.lock().await;
    let ten_secs = time::Duration::from_secs(10);
    tokio::time::sleep(ten_secs).await;
    *n += 1;
    println!("--- end lock_sleep");
}

#[tokio::main]
async fn main() -> Result<(), tokio::task::JoinError> {
    let val = Arc::new(Mutex::new(0));
    let mut v = Vec::new();

    // lock_sleep タスク生成
    let t = tokio::spawn(lock_sleep(val.clone()));
    v.push(t);

    for _ in 0..NUM_TASKS {
        let n = val.clone();
        let t = tokio::spawn(lock_only(n));
        v.push(t);
    }

    for i in v {
        i.await?;
    }

    Ok(())
}
