//! Phase 0 spike: zmq-curve. Throwaway code; findings live in docs/design/spike-zmq-curve.md.
//!
//! libzmq 4.3.5 built from source with CURVE (libsodium), a thin safe wrapper, and the ac2
//! socket design: ROUTER/DEALER ctrl with request ids, XPUB/SUB data with multipart frames,
//! CURVE + ZAP on both, and a client-side latest-wins drain.

#![allow(non_camel_case_types, reason = "ffi mirrors zmq.h names")]

pub mod ctrl;
pub mod data;
pub mod ffi;
pub mod proto;
pub mod zap;
pub mod zmq;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libzmq_has_curve_and_ipc() {
        assert_eq!(zmq::version(), (4, 3, 5));
        assert!(zmq::has("curve"), "libzmq built without CURVE");
        #[cfg(unix)]
        assert!(zmq::has("ipc"));
    }

    #[test]
    fn z85_roundtrip() -> Result<(), zmq::Error> {
        let kp = zmq::CurveKeyPair::generate()?;
        let raw = zmq::z85_decode_key(&kp.public)?;
        assert_eq!(zmq::z85_encode_key(&raw), kp.public);
        Ok(())
    }

    #[test]
    fn frame_roundtrip_and_bounds() {
        use proto::*;
        let h = FrameHeader {
            seq: 7,
            audio_sample: 48_000,
            daemon_incarnation: 1,
            config_rev: 3,
            grid_id: 9,
            kind: FrameKind::Tf,
            n: 3,
            capture_wall_ns: 1,
        };
        let parts = encode_frame("d/m1/tf", &h, &[1.0, -2.5, f32::NAN]);
        let f = decode_frame(&parts).expect("valid frame");
        assert_eq!(f.header, h);
        assert_eq!(f.values[..2], [1.0, -2.5]);
        assert!(f.values[2].is_nan());

        let mut short = parts.to_vec();
        short[2].pop();
        assert_eq!(
            decode_frame(&short),
            Err(FrameError::PayloadLen {
                expected: 12,
                got: 11
            })
        );
        assert_eq!(decode_frame(&parts[..2]), Err(FrameError::Parts(2)));
    }
}
