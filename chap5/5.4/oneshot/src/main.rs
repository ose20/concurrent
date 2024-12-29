use tokio::sync::oneshot;

// 将来のどこかで値が決定される関数
async fn set_val_later(tx: oneshot::Sender<i32>) {
    let ten_secs = std::time::Duration::from_secs(10);
    tokio::time::sleep(ten_secs).await;

    if tx.send(100).is_err() {
        println!("failed to send")
    }
}

#[tokio::main]
async fn main() {
    let outer_rx;
    {
        let (tx, rx) = oneshot::channel();

        tokio::spawn(set_val_later(tx));
        outer_rx = rx;
    }

    match outer_rx.await {
        Ok(n) => {
            println!("n = {}", n);
        }
        Err(e) => {
            println!("failed to receive: {}", e);
            return;
        }
    }
}
