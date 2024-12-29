// 10秒間無駄にワーカスレッドを占有してしまい、他の async タスクを並行に実行できなくなっている
// use std::{thread, time};

// #[tokio::main]
// async fn main() {
// join で終了待機
//     tokio::join!(async move {
//        let ten_secs = time::Duration::from_secs(10);
//        thread::sleep(ten_secs);
//    });
// }

use std::time;

#[tokio::main]
async fn main() {
    tokio::join!(async move {
        let ten_secs = time::Duration::from_secs(10);
        // tokio の sleep を使うことで、ワーカスレッドから退避される
        tokio::time::sleep(ten_secs).await;
    });
}
