use std::sync::{Arc, Mutex};

const NUM_TASKS: usize = 4; // タスク数
const NUM_LOOP: usize = 100000; // ループ数

// ロック獲得中に await しないならば、std::sync::Mutex で問題ない
#[tokio::main]
async fn main() -> Result<(), tokio::task::JoinError> {
    let val = Arc::new(Mutex::new(0));
    let mut join_handles = Vec::new();
    for _ in 0..NUM_TASKS {
        let n = val.clone();

        let t = tokio::spawn(async move {
            for _ in 0..NUM_LOOP {
                let mut n0 = n.lock().unwrap();
                *n0 += 1;
            }
        });

        join_handles.push(t);
    }

    for i in join_handles {
        i.await?;
    }

    println!(
        "COUNT = {} (expected = {})",
        *val.lock().unwrap(),
        NUM_LOOP * NUM_TASKS
    );
    Ok(())
}
