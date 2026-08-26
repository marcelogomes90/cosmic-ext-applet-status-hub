use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

#[derive(Debug)]
pub struct PrivateBus {
    child: Child,
    address: String,
}

impl PrivateBus {
    pub fn start() -> std::io::Result<Self> {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let mut address = String::new();
        BufReader::new(stdout).read_line(&mut address)?;
        let address = address.trim().to_owned();

        if address.is_empty() {
            let _ = child.kill();
            return Err(std::io::Error::other("dbus-daemon printed no address"));
        }

        Ok(Self { child, address })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub async fn connect(&self) -> zbus::Result<zbus::Connection> {
        zbus::connection::Builder::address(self.address.as_str())?
            .build()
            .await
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
