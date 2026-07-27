use thread::*;

fn run_jobs(Receiver rx) -> Result<int, ThreadError> {
    while true {
        let job = recv(rx)?;
        if job == "stop" {
            break;
        }
        print "%s,", job;
    }
    return 0;
}
