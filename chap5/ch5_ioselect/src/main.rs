use futures::{
    future::{BoxFuture, FutureExt},
    task::{waker_ref, ArcWake},
};

use nix::{
    errno::Errno,
    sys::{
        epoll::{
            epoll_create1, epoll_ctl, epoll_wait, EpollCreateFlags, EpollEvent, EpollFlags, EpollOp,
        },
        eventfd::{eventfd, EfdFlags},
    },
    unistd::{read, write},
};

use core::panic;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io::{BufRead, BufReader, BufWriter, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::io::{AsRawFd, RawFd},
    pin::Pin,
    sync::{
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
};

fn write_eventfd(fd: RawFd, n: usize) {
    let ptr = &n as *const usize as *const u8;
    // n をメモリ上の生バイト列としてスライス形式で取得する
    let val = unsafe { std::slice::from_raw_parts(ptr, std::mem::size_of_val(&n)) };
    // fd の直観はチャネル。ここに val を流し込むイメージ
    // 「流し込む」が意味するとこをは、fd が指す具体的なリソースに依存する
    // たとえば file なら末尾に書き込みだったり、eventfd ならカウント値に追加されるとか？
    write(fd, val).unwrap();
}

enum IOOps {
    Add(EpollFlags, RawFd, Waker), // epoll へ追加
    Remove(RawFd),                 // epoll から削除
}

// epfd と event はどちらも RawFd だけど全然違うものらしい

// 1. epfd（epoll のファイルディスクリプタ）
// 生成元: epoll_create1 システムコール。
// 役割:
//  epfd は epoll インスタンスを表すファイルディスクリプタ です。
//  複数のファイルディスクリプタ（ソケット、パイプ、eventfd など）をまとめて監視するために使用されます。
// 主な使い方:
//  - イベントの登録: 監視対象のファイルディスクリプタを epoll_ctl を使って epfd に登録します。
//  - イベントの待機: epoll_wait を使って、登録したファイルディスクリプタに発生したイベントを待機します。
// 特徴:
//  - 複数のファイルディスクリプタを効率的に監視:
//      - ネットワーク接続やファイルI/Oなど、多数のイベントを扱うプログラムで重要。
//  - 状態管理はカーネルが担当:
//      - 登録された監視対象の状態を、ユーザー空間が個別に管理する必要がない。

// 2. event（eventfd のファイルディスクリプタ）
// 生成元: eventfd システムコール。
// 役割:
//  event は プロセス間通信（IPC）やスレッド間通信のためのファイルディスクリプタ です。
//  特定のイベント（通知やシグナル）を発火する目的で使用されます。
// 主な使い方:
//  - 通知の送信:
//      - eventfd_write を使って特定の値（通知）を送信します。
//  - 通知の受信:
//      - eventfd_read を使って通知を受信し、その後に必要な処理を行います。
//  - epoll と組み合わせる:
//      - eventfd を epoll に登録し、タスクやスレッド間の通知を効率よく処理します。
// 特徴:
//  - 通知専用:
//      - eventfd は簡易的な通知の送信・受信専用。
//  - スレッド間やプロセス間での利用:
//      - スレッドセーフなイベント通知を実現。
//  - 軽量でシンプル:
//      - 通知専用の仕組みなので、特定の用途に対して非常に効率的。

// | 特徴               | epfd（epoll fd）                     | event（eventfd）                 |
// |--------------------|-------------------------------------|----------------------------------|
// | **生成システムコール** | `epoll_create1`                   | `eventfd`                       |
// | **目的**            | 複数のファイルディスクリプタを効率的に監視 | 通知（イベント発火）の送受信       |
// | **監視対象**        | ソケット、ファイル、パイプ、`eventfd` など | なし（自身がイベントの発火元）     |
// | **主な操作**        | `epoll_ctl` で対象を登録・管理       | `eventfd_write` / `eventfd_read` |
// | **使い方の規模**    | 大規模な非同期I/Oや多重化             | 単純な通知やシグナル              |
// | **`epoll` との組み合わせ** | `epfd` 自体が `epoll` のインスタンス  | `event` を `epoll` に登録可能     |

// また、この eventfd はほかの全く関係ないプロセスの eventfd とバッティングすることはないらしい
// なぜなら、Linux の eventfd はプロセスやスレッドごとに独立したカーネルリソースとして扱われるから
// 別の観点だが、このプログラム自体は、1つの eventfd を使って処理を実現するように作っていそう

struct IOSelector {
    wakers: Mutex<HashMap<RawFd, Waker>>,
    queue: Mutex<VecDeque<IOOps>>, // IO のキュー
    epfd: RawFd,                   // epoll の fd
    event: RawFd,                  // eventfd の fd
}

impl IOSelector {
    fn new() -> Arc<Self> {
        let s = IOSelector {
            wakers: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            epfd: epoll_create1(EpollCreateFlags::empty()).unwrap(),
            event: eventfd(0, EfdFlags::empty()).unwrap(),
        };
        let result = Arc::new(s);
        let s = result.clone();

        // epoll 用スレッド作成
        std::thread::spawn(move || s.select());

        result
    }

    // epoll で監視するための関数
    fn add_event(
        &self,
        flag: EpollFlags, // epoll のフラグ
        fd: RawFd,        // 監視対象のファイルディスクリプタ
        waker: Waker,
        wakers: &mut HashMap<RawFd, Waker>,
    ) {
        // 各定義のショートカット
        let epoll_add = EpollOp::EpollCtlAdd;
        let epoll_mod = EpollOp::EpollCtlMod;
        let epoll_one = EpollFlags::EPOLLONESHOT;

        // EPOLLONESHOT を指定して、一度イベントが発生すると
        // その fd へのイベントは再設定するまで通知されないようにする
        // ONSHOT にすることでマルチスレッド環境で同じ fd を複数回処理する問題を防げる
        let mut ev = EpollEvent::new(flag | epoll_one, fd as u64);

        // 監視対象に追加
        if let Err(err) = epoll_ctl(self.epfd, epoll_add, fd, &mut ev) {
            match err {
                nix::Error::Sys(Errno::EEXIST) => {
                    // 既に追加されていた場合は再設定
                    // epoll_add じゃなくて epoll_mod にしてる
                    epoll_ctl(self.epfd, epoll_mod, fd, &mut ev).unwrap();
                }
                _ => {
                    panic!("epoll_ctl: {}", err)
                }
            }
        }

        assert!(!wakers.contains_key(&fd));
        wakers.insert(fd, waker);
    }

    // epoll の監視から削除するための関数
    fn rm_event(&self, fd: RawFd, wakers: &mut HashMap<RawFd, Waker>) {
        let epoll_del = EpollOp::EpollCtlDel;
        let mut ev = EpollEvent::new(EpollFlags::empty(), fd as u64);
        epoll_ctl(self.epfd, epoll_del, fd, &mut ev).ok();
        wakers.remove(&fd);
    }

    // 専用のスレッドでファイルディスクリプタの監視を行うための関
    fn select(&self) {
        // 各定義のショートカット
        let epoll_in = EpollFlags::EPOLLIN;
        let epoll_add = EpollOp::EpollCtlAdd;

        // eventfd を epoll の監視対象に追加
        let mut ev = EpollEvent::new(epoll_in, self.event as u64);
        epoll_ctl(self.epfd, epoll_add, self.event, &mut ev).unwrap();

        let mut events = vec![EpollEvent::empty(); 1024];
        // event 発生を監視
        while let Ok(nfds) = epoll_wait(self.epfd, &mut events, -1) {
            let mut t = self.wakers.lock().unwrap();
            for n in 0..nfds {
                if events[n].data() == self.event as u64 {
                    // eventfd の場合、追加、削除要求を処理
                    let mut q = self.queue.lock().unwrap();
                    while let Some(op) = q.pop_front() {
                        match op {
                            // 追加
                            IOOps::Add(flag, fd, waker) => self.add_event(flag, fd, waker, &mut t),
                            IOOps::Remove(fd) => self.rm_event(fd, &mut t),
                        }
                    }
                    let mut buf: [u8; 8] = [0; 8];
                    read(self.event, &mut buf).unwrap(); // eventfd の通知解除
                } else {
                    // 発生したイベントが eventfd じゃない、つまりファイルディスクリプタの場合の処理
                    // 実行キューに追加
                    let data = events[n].data() as i32;
                    let waker = t.remove(&data).unwrap();
                    waker.wake_by_ref();
                }
            }
        }
    }

    // ファイルディスクリプタ登録用関数
    fn register(&self, flags: EpollFlags, fd: RawFd, waker: Waker) {
        let mut q = self.queue.lock().unwrap();
        q.push_back(IOOps::Add(flags, fd, waker));
        // eventfd は内部的に 64 ビットの整数カウンタを持っているので 1 を使うことが多い
        // 多分決まりはない？
        // write でここに指定した値が加算される
        // read の時に 0 にリセットされ
        // epoll と連携してるとき、eventfd のカウンタが 0 から
        write_eventfd(self.event, 1);
    }

    // ファイルディスクリプタ削除用関数
    fn unregister(&self, fd: RawFd) {
        let mut q = self.queue.lock().unwrap();
        q.push_back(IOOps::Remove(fd));
        write_eventfd(self.event, 1);
    }
}

struct AsyncListener {
    listener: TcpListener,
    selector: Arc<IOSelector>,
}

impl AsyncListener {
    fn listen(addr: &str, selector: Arc<IOSelector>) -> AsyncListener {
        // リッスンアドレスを指定
        let listener = TcpListener::bind(addr).unwrap();

        // ノンブロッキングに指定
        // ブロッキングだと、アクセプトすべきコネクションがくるまで停止する
        // ノンブロッキングならアクセプトすべきコネクションがない場合は即座にエラーを投げて停止する
        listener.set_nonblocking(true).unwrap();

        AsyncListener { listener, selector }
    }

    // コネクションをアクセプトするための Future をリターン
    fn accept(&self) -> Accept {
        Accept { listener: self }
    }
}

impl Drop for AsyncListener {
    fn drop(&mut self) {
        self.selector.unregister(self.listener.as_raw_fd());
    }
}

// 非同期アクセプト用 Future の実装
// この Future ではノンブロッキングにアクセプトを実行し、アクセプトできた場合は読み込みと
// 書き込みストリーム及びアドレスをリターンし終了する
// アクセプトすべきコネクションがない場合はリッスンソケットを epoll に監視対象として追加して実行を中断する

struct Accept<'a> {
    listener: &'a AsyncListener,
}

impl<'a> Future for Accept<'a> {
    // 返り値の型
    type Output = (
        AsyncReader,          // 非同期読み込みストリーム
        BufWriter<TcpStream>, // 書き込みストリーム
        SocketAddr,           // アドレス
    );

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // アクセプトをノンブロッキングで実行
        match self.listener.listener.accept() {
            Ok((stream, addr)) => {
                // アクセプトした倍は
                // 読み込みと書き込み用オブジェクト及びアドレスをリターン
                let stream0 = stream.try_clone().unwrap();
                Poll::Ready((
                    AsyncReader::new(stream0, self.listener.selector.clone()),
                    BufWriter::new(stream),
                    addr,
                ))
            }
            Err(err) => {
                // アクセプトすべきコネクションがない場合は epoll に登録
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    self.listener.selector.register(
                        EpollFlags::EPOLLIN,
                        self.listener.listener.as_raw_fd(),
                        cx.waker().clone(),
                    );
                    Poll::Pending
                } else {
                    panic!("accept: {}", err)
                }
            }
        }
    }
}

struct AsyncReader {
    fd: RawFd,
    reader: BufReader<TcpStream>,
    selector: Arc<IOSelector>,
}

impl AsyncReader {
    fn new(stream: TcpStream, selector: Arc<IOSelector>) -> AsyncReader {
        // ノンブロッキングに設定
        stream.set_nonblocking(true).unwrap();
        AsyncReader {
            fd: stream.as_raw_fd(),
            reader: BufReader::new(stream),
            selector,
        }
    }

    // 1行読み込みのための Future をリターン
    fn read_line(&mut self) -> ReadLine {
        ReadLine { reader: self }
    }
}

impl Drop for AsyncReader {
    fn drop(&mut self) {
        self.selector.unregister(self.fd);
    }
}

struct ReadLine<'a> {
    reader: &'a mut AsyncReader,
}

impl<'a> Future for ReadLine<'a> {
    type Output = Option<String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut line = String::new();

        // 非同期読み込み
        match self.reader.reader.read_line(&mut line) {
            Ok(0) => Poll::Ready(None),       // コネクションクローズ
            Ok(_) => Poll::Ready(Some(line)), // 1行読み込み成功
            Err(err) => {
                // 読み込みできない場合は epoll に登録
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    self.reader.selector.register(
                        EpollFlags::EPOLLIN,
                        self.reader.fd,
                        cx.waker().clone(),
                    );
                    Poll::Pending
                } else {
                    Poll::Ready(None)
                }
            }
        }
    }
}

struct Task {
    // 実行するコルーチン
    future: Mutex<BoxFuture<'static, ()>>,
    // Executor へスケジューリングするためのチャネル
    sender: SyncSender<Arc<Task>>,
}

impl ArcWake for Task {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        // 自身をスケジューリング
        let self0 = arc_self.clone();
        arc_self.sender.send(self0).unwrap();
    }
}

struct Executor {
    // 実行キュー
    sender: SyncSender<Arc<Task>>,
    receiver: Receiver<Arc<Task>>,
}

impl Executor {
    fn new() -> Self {
        // チャネルを生成
        let (sender, receiver) = sync_channel(1024);
        Executor {
            sender: sender.clone(),
            receiver,
        }
    }

    // 新たに Task を生成するための Spawner を作成
    fn get_spawner(&self) -> Spawner {
        Spawner {
            sender: self.sender.clone(),
        }
    }

    fn run(&self) {
        // チャネルから Task を受信して順に実行
        while let Ok(task) = self.receiver.recv() {
            // コンテキストを生成
            let mut future = task.future.lock().unwrap();
            let waker = waker_ref(&task);
            let mut ctx = Context::from_waker(&waker);
            // poll 呼び出し実行
            let _ = future.as_mut().poll(&mut ctx);
        }
    }
}

struct Spawner {
    sender: SyncSender<Arc<Task>>,
}

impl Spawner {
    // 今回のコードは Output = Option<String> のやつもあったけどそれはここには関係ないのかな
    fn spawn(&self, future: impl Future<Output = ()> + 'static + Send) {
        let future = future.boxed();
        let task = Arc::new(Task {
            future: Mutex::new(future),
            sender: self.sender.clone(),
        });

        // 実行キューにえんきゅー
        self.sender.send(task).unwrap();
    }
}

fn main() {
    let executor = Executor::new();
    let selector = IOSelector::new();
    let spawner = executor.get_spawner();

    let server = async move {
        let listener = AsyncListener::listen("127.0.0.1:10000", selector.clone());

        loop {
            // 非同期コネクションアクセプト
            let (mut reader, mut writer, addr) = listener.accept().await;
            println!("accept: {}", addr);

            // コネクションごとにタスクを作成
            spawner.spawn(async move {
                // 1行非同期読み込み
                while let Some(buf) = reader.read_line().await {
                    print!("read: {}, {}", addr, buf);
                    writer.write_all(buf.as_bytes()).unwrap();
                    writer.flush().unwrap();
                }
                println!("close: {}", addr);
            });
        }
    };

    // タスクを生成して実行
    executor.get_spawner().spawn(server);
    executor.run();
}
