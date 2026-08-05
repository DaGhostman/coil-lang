use thread::{Sender, Thread, channel, join, send, spawn};
use pool::worker::run_jobs;

class Worker {
    thread: Thread,
    tx: Sender,
}

impl Worker {
    fn submit(string job) {
        send(self.tx, job)?;
    }

    fn join() {
        join(self.thread)?;
    }
}

fn main() {
    let pair = channel()?;
    let t = spawn(run_jobs, pair[1])?;
    let w = new Worker(t, pair[0]);
    w.submit("a")?;
    w.submit("b")?;
    w.submit("stop")?;
    w.join()?;
}
