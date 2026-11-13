//! The command layer against a simulated scanner.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use pakon_proto::setup::{self, Seq, LED_PERIOD_F135};
use pakon_proto::frame::PacketType;
use pakon_scan::cmd::{self, Pic};
use pakon_scan::link::Link;
use pakon_scan::ScanError;

/// Answers every frame like a healthy scanner, with per-(type, addr)
/// queues of overrides (status or flags byte, or a whole reply).
#[derive(Default)]
struct Sim {
    sent: Vec<Vec<u8>>,
    pending: VecDeque<Vec<u8>>,
    status: HashMap<(u8, u8), VecDeque<u8>>,
    replies: HashMap<(u8, u8), VecDeque<Vec<u8>>>,
    images: VecDeque<Vec<u8>>,
}

impl Sim {
    fn queue_status(&mut self, ty: u8, addr: u8, v: &[u8]) {
        self.status.entry((ty, addr)).or_default().extend(v);
    }
}

impl Link for Sim {
    fn send_cmd(&mut self, wire: &[u8], _: Duration) -> Result<usize, ScanError> {
        self.sent.push(wire.to_vec());
        let (ty, a) = (wire[0], wire[2]);
        if let Some(r) = self.replies.get_mut(&(ty, a)).and_then(|q| q.pop_front()) {
            self.pending.push_back(r);
            return Ok(wire.len());
        }
        let st = self.status.get_mut(&(ty, a)).and_then(|q| q.pop_front()).unwrap_or(0);
        let r = match ty {
            1 => {
                let n = wire[3] as usize;
                let mut r = vec![1, (2 + n) as u8, a, st];
                r.extend(std::iter::repeat_n(0, n));
                r
            }
            3 => vec![3, 2, a, st],
            _ => vec![7, 2, a, st],
        };
        self.pending.push_back(r);
        Ok(wire.len())
    }
    fn recv_cmd(&mut self, buf: &mut [u8], _: Duration) -> Result<usize, ScanError> {
        let r = self.pending.pop_front().ok_or(ScanError::Timeout)?;
        buf[..r.len()].copy_from_slice(&r);
        Ok(r.len())
    }
    fn recv_image(&mut self, buf: &mut [u8], _: Duration) -> Result<usize, ScanError> {
        let c = self.images.pop_front().ok_or(ScanError::Timeout)?;
        buf[..c.len()].copy_from_slice(&c);
        Ok(c.len())
    }
    fn control(&mut self, _: &pakon_usb::device::Control, _: Duration) -> Result<Vec<u8>, ScanError> {
        Ok(vec![])
    }
}

const T: Duration = Duration::from_millis(100);

#[test]
fn init_sequence_runs_frame_for_frame() {
    let mut seq = Seq::new();
    setup::init(&mut seq, 0x20, 0x24, LED_PERIOD_F135);
    let mut sim = Sim::default();
    cmd::run_seq(&mut sim, &seq, T).unwrap();
    let want: Vec<Vec<u8>> = seq.frames().iter().map(|f| f.to_bytes()).collect();
    assert_eq!(sim.sent, want);
}

#[test]
fn busy_poll_repeats_until_free() {
    let mut seq = Seq::new();
    seq.push(PacketType::ReadStatus, &[0x20]);
    let mut sim = Sim::default();
    sim.queue_status(3, 0x20, &[1, 1, 1, 0]);
    cmd::run_seq(&mut sim, &seq, T).unwrap();
    assert_eq!(sim.sent.len(), 4);
    let mut sim = Sim::default();
    sim.queue_status(3, 0x20, &[0x20]);
    assert_eq!(cmd::run_seq(&mut sim, &seq, T), Err(ScanError::Status(0x20)));
    // 0x04 is an error from a controller, not from the host
    let mut sim = Sim::default();
    sim.queue_status(3, 0x20, &[0x04]);
    assert_eq!(cmd::run_seq(&mut sim, &seq, T), Err(ScanError::Status(0x04)));
    let mut host = Seq::new();
    host.push(PacketType::ReadStatus, &[0x10]);
    let mut sim = Sim::default();
    sim.queue_status(3, 0x10, &[0x04]);
    assert!(cmd::run_seq(&mut sim, &host, T).is_ok());
}

#[test]
fn write_retries_and_errors() {
    let mut seq = Seq::new();
    seq.push(PacketType::Write, &[0x24, 0x01, 0xA2, 0x00]);
    let mut sim = Sim::default();
    sim.queue_status(2, 0x24, &[3, 6, 0]);
    cmd::run_seq(&mut sim, &seq, T).unwrap();
    assert_eq!(sim.sent.len(), 3);
    let mut sim = Sim::default();
    sim.queue_status(2, 0x24, &[9, 9, 9]);
    assert_eq!(cmd::run_seq(&mut sim, &seq, T), Err(ScanError::Status(9)));
    let mut sim = Sim::default();
    sim.queue_status(2, 0x24, &[1]);
    assert_eq!(cmd::run_seq(&mut sim, &seq, T), Err(ScanError::Status(1)));
    assert_eq!(sim.sent.len(), 1);
    let mut sim = Sim::default();
    sim.queue_status(2, 0x24, &[8]);
    assert!(cmd::run_seq(&mut sim, &seq, T).is_ok());
}

#[test]
fn unsafe_sequences_send_nothing() {
    for addr in [0x22u8, 0x26, 0x42, 0x46] {
        let mut seq = Seq::new();
        seq.push(PacketType::Write, &[0x20, 0x01, 0x80, 0x00]);
        seq.push(PacketType::Command, &[addr, 0x00, 0x0C]);
        let mut sim = Sim::default();
        assert!(matches!(cmd::run_seq(&mut sim, &seq, T), Err(ScanError::Param(_))));
        assert!(sim.sent.is_empty(), "nothing may go out before the check");
    }
}

#[test]
fn pic_probe() {
    let mut sim = Sim::default();
    assert_eq!(cmd::probe_pic(&mut sim, 0x20, T), Pic::Present);
    assert_eq!(sim.sent, vec![vec![4, 3, 0x20, 0, 0]]);
    sim.queue_status(4, 0x40, &[1]);
    assert_eq!(cmd::probe_pic(&mut sim, 0x40, T), Pic::Absent);
    sim.queue_status(4, 0x44, &[9]);
    assert_eq!(cmd::probe_pic(&mut sim, 0x44, T), Pic::Error(9));
    // a reply from the wrong address concludes nothing
    sim.replies.entry((4, 0x24)).or_default().push_back(vec![7, 2, 0x20, 0]);
    assert_eq!(cmd::probe_pic(&mut sim, 0x24, T), Pic::Error(0xFF));
}

#[test]
fn event_service_acks_low_then_scn() {
    let mut sim = Sim::default();
    // idle: one host poll, nothing else
    let ev = cmd::service_events(&mut sim, 0x20, 0x24, T, None).unwrap();
    assert_eq!((ev.host_flags, ev.serviced, sim.sent.len()), (0, 0, 1));

    let mut sim = Sim::default();
    sim.queue_status(3, 0x10, &[0x80]);
    sim.replies.entry((1, 0x20)).or_default().push_back(vec![1, 3, 0x20, 0x80, 0x02]);
    sim.replies.entry((1, 0x24)).or_default().push_back(vec![1, 3, 0x24, 0x00, 0x00]);
    let ev = cmd::service_events(&mut sim, 0x20, 0x24, T, None).unwrap();
    assert_eq!((ev.host_flags, ev.serviced, ev.low_status), (0x80, 1, 0x02));
    let mut seq = Seq::new();
    setup::event_followup(&mut seq, 0x20, 0x02, true);
    let mut want = vec![vec![3, 1, 0x10], setup::event_read_frame(0x20).to_bytes()];
    want.extend(seq.frames().iter().map(|f| f.to_bytes()));
    want.push(setup::event_read_frame(0x24).to_bytes());
    assert_eq!(sim.sent, want);
}

fn image_chunk(level: u16) -> Vec<u8> {
    (0..pakon_scan::calib::CHUNK / 2).flat_map(|_| level.to_le_bytes()).collect()
}

fn f135_state() -> setup::SetupState {
    let mut seq = Seq::new();
    setup::init(&mut seq, 0x20, 0x24, LED_PERIOD_F135)
}

#[test]
fn scan_loop_stops_after_the_strip() {
    use pakon_scan::scan::{self, WHITE_FRAC_CODE_PCT, WHITE_THRESH_CODE};
    let mut sim = Sim::default();
    sim.images.extend((0..5).map(|_| image_chunk(2400))); // dim leader
    sim.images.extend((0..3).map(|_| image_chunk(60000))); // open gate before the film
    sim.images.extend((0..10).map(|_| image_chunk(15000))); // film
    sim.images.extend((0..40).map(|_| image_chunk(60000))); // open gate after it
    let mut img = Vec::new();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let mut roll = pakon_proto::dx::Roll::new();
    let mut lines = Vec::new();
    let st = f135_state();
    let s = scan::scan_loop(&mut sim, &st, &mut img, T, 0, (WHITE_THRESH_CODE, WHITE_FRAC_CODE_PCT), &stop, &mut roll, &mut |l| lines.push(l.to_string()));
    assert!(s.film_seen);
    assert_eq!(s.reads, 5 + 3 + 10 + 24, "stops on the 24th open-gate chunk after film");
    assert_eq!(img.len() as u64, s.bytes);
    assert_eq!(sim.images.len(), 16);
    assert!(lines.iter().any(|l| l.contains("film detected")));
    assert!(lines.iter().any(|l| l.contains("end-of-roll")));
}

#[test]
fn scan_loop_honours_stop_and_no_film() {
    use pakon_scan::scan;
    let mut sim = Sim::default();
    let stop = std::sync::atomic::AtomicBool::new(true);
    let mut roll = pakon_proto::dx::Roll::new();
    let st = f135_state();
    let s = scan::scan_loop(&mut sim, &st, &mut Vec::new(), T, 0, (40000, 60), &stop, &mut roll, &mut |_| {});
    assert_eq!((s.reads, s.film_seen), (0, false));
    assert!(sim.sent.is_empty());
}

#[test]
fn teardown_runs_on_drop() {
    use pakon_scan::scan::Teardown;
    let st = f135_state();
    let mut want = Seq::new();
    setup::teardown(&mut want, &st);
    let want: Vec<Vec<u8>> = want.frames().iter().map(|f| f.to_bytes()).collect();
    let mut sim = Sim::default();
    {
        let _guard = Teardown::new(&mut sim, st, T);
        // an error or a panic here leaves the scope
    }
    assert_eq!(sim.sent, want);
    let mut sim = Sim::default();
    Teardown::new(&mut sim, st, T).finish().unwrap();
    assert_eq!(sim.sent, want, "finish sends it once");
}
