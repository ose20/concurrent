use std::{
    collections::LinkedList,
    sync::{Arc, Condvar, Mutex},
};

use crate::semaphore::Semaphore;

#[derive(Clone)]
pub struct Sender<T> {
    semaphore: Arc<Semaphore>,      // 有限性を実現するセマフォ
    buf: Arc<Mutex<LinkedList<T>>>, // queue
    cond: Arc<Condvar>,
}

impl<T: Send> Sender<T> {
    pub fn send(&self, data: T) {
        self.semaphore.wait();
        let mut buf = self.buf.lock().unwrap();
        buf.push_back(data);
        self.cond.notify_one();
    }
}

pub struct Receiver<T> {
    semaphore: Arc<Semaphore>,
    buf: Arc<Mutex<LinkedList<T>>>,
    cond: Arc<Condvar>,
}

impl<T> Receiver<T> {
    pub fn recv(&self) -> T {
        let mut buf = self.buf.lock().unwrap();
        loop {
            if let Some(data) = buf.pop_front() {
                self.semaphore.post();
                return data;
            }
            buf = self.cond.wait(buf).unwrap();
        }
    }
}

pub fn channel<T>(max: isize) -> (Sender<T>, Receiver<T>) {
    assert!(max > 0);
    let semaphore = Arc::new(Semaphore::new(max));
    let buf = Arc::new(Mutex::new(LinkedList::new()));
    let cond = Arc::new(Condvar::new());
    let tx = Sender {
        semaphore: semaphore.clone(),
        buf: buf.clone(),
        cond: cond.clone(),
    };
    let rx = Receiver {
        semaphore,
        buf,
        cond,
    };
    (tx, rx)
}
