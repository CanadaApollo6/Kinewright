struct S4Child(std::process::Child);

impl Drop for S4Child {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.0.kill();
        let deadline = Instant::now() + Duration::from_secs(30);
        while self
            .0
            .try_wait()
            .unwrap_or_else(|error| {
                eprintln!("S4 cleanup failed: {error}");
                None
            })
            .is_none()
        {
            if Instant::now() >= deadline {
                eprintln!("S4 cleanup failed: child {} was not reaped", self.0.id());
                return;
            }
            thread::sleep(SAMPLE);
        }
    }
}

trait S4Output {
    fn s4_output(&mut self) -> std::io::Result<std::process::Output>;
}

impl S4Output for std::process::Command {
    fn s4_output(&mut self) -> std::io::Result<std::process::Output> {
        let temp = TempDirectory::new("s4-child");
        let stdout = temp.path("stdout");
        let stderr = temp.path("stderr");
        let mut child = S4Child(
            self.stdout(std::fs::File::create(&stdout)?)
                .stderr(std::fs::File::create(&stderr)?)
                .spawn()?,
        );
        let deadline = Instant::now() + Duration::from_secs(600);
        let status = loop {
            if let Some(status) = child.0.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "S4 child exceeded 600 s",
                ));
            }
            thread::sleep(SAMPLE);
        };
        Ok(std::process::Output {
            status,
            stdout: std::fs::read(stdout)?,
            stderr: std::fs::read(stderr)?,
        })
    }
}
