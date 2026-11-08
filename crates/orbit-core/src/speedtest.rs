//! Internet speed tests, run by Ookla's Speedtest CLI against Speedtest.net's servers. The platform
//! starts the program and hands over each line it prints (`--format=jsonl --progress=yes`). This
//! module reads them and keeps what the test has measured so far. It never reads the addresses the
//! program also prints, so they can't be kept or shown.

use serde::Deserialize;

/// Where a test is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Nothing run yet.
    #[default]
    Idle,
    /// Started, finding a server.
    Starting,
    Ping,
    Download,
    Upload,
    Done,
    Failed,
}

/// A test so far, or finished.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Test {
    pub phase: Phase,
    /// How far through the current phase, from 0 to 1.
    pub progress: f32,
    /// Milliseconds.
    pub ping: Option<f64>,
    pub jitter: Option<f64>,
    /// Megabits a second.
    pub download: Option<f64>,
    pub upload: Option<f64>,
    /// Each progress report's speed, in megabits a second, for drawing the test as it went.
    pub download_samples: Vec<f32>,
    pub upload_samples: Vec<f32>,
    /// Percent, when the network allows it to be measured.
    pub packet_loss: Option<f64>,
    /// "BT Ireland", "Dublin, Ireland".
    pub server: String,
    pub location: String,
    pub isp: String,
    /// The result's page on Speedtest.net.
    pub url: Option<String>,
    pub problem: Option<String>,
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    isp: Option<String>,
    #[serde(default)]
    server: Option<Server>,
    #[serde(default)]
    ping: Option<Ping>,
    #[serde(default)]
    download: Option<Transfer>,
    #[serde(default)]
    upload: Option<Transfer>,
    #[serde(default, rename = "packetLoss")]
    packet_loss: Option<f64>,
    #[serde(default)]
    result: Option<Shared>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct Server {
    #[serde(default)]
    name: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    country: String,
}

#[derive(Deserialize)]
struct Ping {
    #[serde(default)]
    latency: Option<f64>,
    #[serde(default)]
    jitter: Option<f64>,
    #[serde(default)]
    progress: Option<f32>,
}

#[derive(Deserialize)]
struct Transfer {
    /// Bytes a second.
    #[serde(default)]
    bandwidth: Option<f64>,
    #[serde(default)]
    progress: Option<f32>,
}

#[derive(Deserialize)]
struct Shared {
    #[serde(default)]
    url: Option<String>,
}

/// Bytes a second as megabits a second, as Speedtest.net shows them.
pub fn megabits(bytes_per_second: f64) -> f64 {
    bytes_per_second / 125_000.0
}

/// "350", "87.4", "9.62": three figures at most, as speeds are usually written.
pub fn format_speed(megabits: f64) -> String {
    if megabits >= 100.0 {
        format!("{megabits:.0}")
    } else if megabits >= 10.0 {
        format!("{megabits:.1}")
    } else {
        format!("{megabits:.2}")
    }
}

/// The most progress reports kept for drawing, which is far more than a test sends.
const MOST_SAMPLES: usize = 400;

impl Test {
    /// A new test, just started.
    pub fn starting() -> Test {
        Test { phase: Phase::Starting, ..Test::default() }
    }

    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Starting | Phase::Ping | Phase::Download | Phase::Upload)
    }

    /// Takes in one line the program printed. Lines it doesn't know are passed over.
    pub fn read(&mut self, line: &str) {
        let Ok(line) = serde_json::from_str::<Line>(line.trim()) else { return };
        match line.kind.as_str() {
            "testStart" => {
                if let Some(isp) = line.isp {
                    self.isp = isp;
                }
                if let Some(server) = line.server {
                    self.server = server.name;
                    self.location = [server.location, server.country].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(", ");
                }
            }
            "ping" => {
                if let Some(ping) = line.ping {
                    self.phase = Phase::Ping;
                    self.ping = ping.latency.or(self.ping);
                    self.jitter = ping.jitter.or(self.jitter);
                    self.progress = ping.progress.unwrap_or(0.0).clamp(0.0, 1.0);
                }
            }
            "download" => {
                if let Some(transfer) = line.download {
                    self.phase = Phase::Download;
                    self.take(transfer, true);
                }
            }
            "upload" => {
                if let Some(transfer) = line.upload {
                    self.phase = Phase::Upload;
                    self.take(transfer, false);
                }
            }
            "result" => {
                if let Some(ping) = line.ping {
                    self.ping = ping.latency.or(self.ping);
                    self.jitter = ping.jitter.or(self.jitter);
                }
                if let Some(bandwidth) = line.download.and_then(|transfer| transfer.bandwidth) {
                    self.download = Some(megabits(bandwidth));
                }
                if let Some(bandwidth) = line.upload.and_then(|transfer| transfer.bandwidth) {
                    self.upload = Some(megabits(bandwidth));
                }
                self.packet_loss = line.packet_loss;
                self.url = line.result.and_then(|shared| shared.url).filter(|url| url.starts_with("https://www.speedtest.net/"));
                self.phase = Phase::Done;
                self.progress = 1.0;
            }
            "log" if line.level.as_deref() == Some("error") => {
                self.fail(line.message.as_deref().unwrap_or("The test stopped."));
            }
            _ if line.error.is_some() => self.fail(line.error.as_deref().unwrap_or_default()),
            _ => {}
        }
    }

    fn take(&mut self, transfer: Transfer, download: bool) {
        self.progress = transfer.progress.unwrap_or(0.0).clamp(0.0, 1.0);
        // A reading that isn't a speed, such as a negative one as a phase starts, is passed over.
        let Some(bandwidth) = transfer.bandwidth.filter(|bandwidth| bandwidth.is_finite() && *bandwidth > 0.0) else { return };
        let speed = megabits(bandwidth);
        let (value, samples) = if download { (&mut self.download, &mut self.download_samples) } else { (&mut self.upload, &mut self.upload_samples) };
        *value = Some(speed);
        if samples.len() < MOST_SAMPLES {
            samples.push(speed as f32);
        }
    }

    /// Stopped short: says why, in the program's words if it gave any.
    pub fn fail(&mut self, why: &str) {
        if self.phase == Phase::Done {
            return;
        }
        let why = why.trim();
        self.phase = Phase::Failed;
        self.problem = Some(if why.is_empty() { "The test stopped.".to_string() } else { why.to_string() });
    }

    /// What was measured, as a line of text to copy.
    pub fn summary(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if let Some(download) = self.download {
            parts.push(format!("Download {} Mbps", format_speed(download)));
        }
        if let Some(upload) = self.upload {
            parts.push(format!("Upload {} Mbps", format_speed(upload)));
        }
        if let Some(ping) = self.ping {
            parts.push(format!("Ping {ping:.0} ms"));
        }
        if !self.server.is_empty() {
            parts.push(format!("{} ({})", self.server, self.location));
        }
        let mut text = parts.join(", ");
        if let Some(url) = &self.url {
            text.push_str(&format!(". {url}"));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines as Speedtest CLI 1.2.0 prints them, with made-up addresses.
    const RUN: &str = r#"{"type":"testStart","timestamp":"2026-11-08T01:01:02Z","isp":"Example ISP","interface":{"internalIp":"10.0.0.2","name":"","macAddr":"00:00:00:00:00:00","isVpn":false,"externalIp":"192.0.2.7"},"server":{"id":1,"host":"speed.example.net","port":8080,"name":"Example Net","location":"Dublin","country":"Ireland","ip":"198.51.100.4"}}
{"type":"ping","timestamp":"2026-11-08T01:01:02Z","ping":{"jitter":0.000,"latency":5.550,"progress":0.200}}
{"type":"ping","timestamp":"2026-11-08T01:01:02Z","ping":{"jitter":0.161,"latency":5.433,"progress":1.000}}
{"type":"download","timestamp":"2026-11-08T01:01:02Z","download":{"bandwidth":28968105,"bytes":6866368,"elapsed":237,"progress":0.016}}
{"type":"download","timestamp":"2026-11-08T01:01:03Z","download":{"bandwidth":36074848,"bytes":17593884,"elapsed":487,"latency":{"iqm":29.923},"progress":0.5}}
{"type":"upload","timestamp":"2026-11-08T01:01:23Z","upload":{"bandwidth":12812621,"bytes":121523374,"elapsed":9504,"latency":{"iqm":56.628},"progress":0.25}}
{"type":"result","timestamp":"2026-11-08T01:01:24Z","ping":{"jitter":0.161,"latency":5.589,"low":5.433,"high":5.815},"download":{"bandwidth":43842723,"bytes":492162660,"elapsed":11509},"upload":{"bandwidth":12812621,"bytes":121523374,"elapsed":9504},"packetLoss":0,"isp":"Example ISP","server":{"id":1,"name":"Example Net","location":"Dublin","country":"Ireland"},"result":{"id":"abc","url":"https://www.speedtest.net/result/c/abc","persisted":true}}"#;

    #[test]
    fn a_test_is_followed_line_by_line() {
        let mut test = Test::starting();
        let mut lines = RUN.lines();
        test.read(lines.next().unwrap());
        assert_eq!((test.server.as_str(), test.location.as_str(), test.isp.as_str()), ("Example Net", "Dublin, Ireland", "Example ISP"));
        test.read(lines.next().unwrap());
        test.read(lines.next().unwrap());
        assert_eq!(test.phase, Phase::Ping);
        assert_eq!(test.ping, Some(5.433));
        test.read(lines.next().unwrap());
        test.read(lines.next().unwrap());
        assert_eq!(test.phase, Phase::Download);
        assert_eq!(test.progress, 0.5);
        assert_eq!(test.download_samples.len(), 2);
        assert_eq!(format_speed(test.download.unwrap()), "289");
        test.read(lines.next().unwrap());
        assert_eq!(test.phase, Phase::Upload);
        assert!(test.running());
        test.read(lines.next().unwrap());
        assert_eq!(test.phase, Phase::Done);
        assert_eq!(format_speed(test.download.unwrap()), "351");
        assert_eq!(format_speed(test.upload.unwrap()), "103");
        assert_eq!(test.packet_loss, Some(0.0));
        assert_eq!(test.url.as_deref(), Some("https://www.speedtest.net/result/c/abc"));
        assert_eq!(test.summary(), "Download 351 Mbps, Upload 103 Mbps, Ping 6 ms, Example Net (Dublin, Ireland). https://www.speedtest.net/result/c/abc");
    }

    #[test]
    fn addresses_are_never_kept() {
        let mut test = Test::starting();
        for line in RUN.lines() {
            test.read(line);
        }
        let everything = format!("{test:?}");
        for address in ["10.0.0.2", "192.0.2.7", "198.51.100.4", "00:00:00:00:00:00", "speed.example.net"] {
            assert!(!everything.contains(address), "{address}");
        }
    }

    #[test]
    fn errors_and_other_lines() {
        let mut test = Test::starting();
        test.read("not json");
        test.read(r#"{"type":"log","level":"info","message":"hello"}"#);
        assert!(test.running());
        test.read(r#"{"type":"log","timestamp":"x","message":"Configuration - Couldn't connect to server (NoData)","level":"error"}"#);
        assert_eq!(test.phase, Phase::Failed);
        assert_eq!(test.problem.as_deref(), Some("Configuration - Couldn't connect to server (NoData)"));
        test.read(r#"{"type":"upload","upload":{"bandwidth":-1250000000,"progress":0.0}}"#);
        assert!(test.upload_samples.is_empty() && test.upload.is_none(), "a negative reading is no speed");
        assert_eq!(format_speed(9.6234), "9.62");
        assert_eq!(format_speed(87.44), "87.4");
    }
}
