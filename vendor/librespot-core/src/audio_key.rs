use std::{collections::HashMap, io::Write, time::Duration};

use byteorder::{BigEndian, ByteOrder, WriteBytesExt};
use bytes::Bytes;
use thiserror::Error;
use tokio::sync::{broadcast, oneshot};

use crate::{Error, FileId, SpotifyId, packet::PacketType, util::SeqGenerator};

#[derive(Debug, Hash, PartialEq, Eq, Copy, Clone)]
pub struct AudioKey(pub [u8; 16]);

// Cadence patch: the refusal keeps Spotify's two-byte reason, and every
// answer is broadcast to `AudioKeyManager::outcomes`, so the host can tell an
// account Spotify won't unlock songs for from one song that failed. Upstream
// only logs the reason and lets playback continue without decryption.
#[derive(Debug, Error)]
pub enum AudioKeyError {
    #[error("audio key error {0:#06x}")]
    AesKey(u16),
    #[error("other end of channel disconnected")]
    Channel,
    #[error("unexpected packet type {0}")]
    Packet(u8),
    #[error("sequence {0} not pending")]
    Sequence(u32),
    #[error("audio key response timeout")]
    Timeout,
}

impl From<AudioKeyError> for Error {
    fn from(err: AudioKeyError) -> Self {
        match err {
            AudioKeyError::AesKey(_) => Error::unavailable(err),
            AudioKeyError::Channel => Error::aborted(err),
            AudioKeyError::Sequence(_) => Error::aborted(err),
            AudioKeyError::Packet(_) => Error::unimplemented(err),
            AudioKeyError::Timeout => Error::aborted(err),
        }
    }
}

/// Spotify's answer to one key request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioKeyOutcome {
    pub track: SpotifyId,
    /// `None` when the key was granted, otherwise Spotify's reason code.
    pub refused: Option<u16>,
}

const OUTCOME_CAPACITY: usize = 16;

component! {
    AudioKeyManager : AudioKeyManagerInner {
        sequence: SeqGenerator<u32> = SeqGenerator::new(0),
        pending: HashMap<u32, (SpotifyId, oneshot::Sender<Result<AudioKey, Error>>)> = HashMap::new(),
        outcomes: broadcast::Sender<AudioKeyOutcome> = broadcast::channel(OUTCOME_CAPACITY).0,
    }
}

impl AudioKeyManager {
    pub(crate) fn dispatch(&self, cmd: PacketType, mut data: Bytes) -> Result<(), Error> {
        let seq = BigEndian::read_u32(data.split_to(4).as_ref());

        let (track, sender) = self
            .lock(|inner| inner.pending.remove(&seq))
            .ok_or(AudioKeyError::Sequence(seq))?;

        match cmd {
            PacketType::AesKey => {
                let mut key = [0u8; 16];
                key.copy_from_slice(data.as_ref());
                self.announce(track, None);
                sender
                    .send(Ok(AudioKey(key)))
                    .map_err(|_| AudioKeyError::Channel)?
            }
            PacketType::AesKeyError => {
                let code = match data.as_ref() {
                    [high, low, ..] => u16::from_be_bytes([*high, *low]),
                    _ => 0,
                };
                error!("error audio key {:x} {:x}", code >> 8, code & 0xff);
                self.announce(track, Some(code));
                sender
                    .send(Err(AudioKeyError::AesKey(code).into()))
                    .map_err(|_| AudioKeyError::Channel)?
            }
            _ => {
                trace!("Did not expect {cmd:?} AES key packet with data {data:#?}");
                return Err(AudioKeyError::Packet(cmd as u8).into());
            }
        }

        Ok(())
    }

    /// Every later answer to a key request, granted or refused.
    pub fn outcomes(&self) -> broadcast::Receiver<AudioKeyOutcome> {
        self.lock(|inner| inner.outcomes.subscribe())
    }

    fn announce(&self, track: SpotifyId, refused: Option<u16>) {
        // Nobody listening is fine; the request's own reply still goes out.
        let _ = self.lock(|inner| inner.outcomes.send(AudioKeyOutcome { track, refused }));
    }

    pub async fn request(&self, track: SpotifyId, file: FileId) -> Result<AudioKey, Error> {
        let (tx, rx) = oneshot::channel();

        let seq = self.lock(move |inner| {
            let seq = inner.sequence.get();
            inner.pending.insert(seq, (track, tx));
            seq
        });

        self.send_key_request(seq, track, file)?;
        const KEY_RESPONSE_TIMEOUT: Duration = Duration::from_millis(1500);
        match tokio::time::timeout(KEY_RESPONSE_TIMEOUT, rx).await {
            Err(_) => {
                error!("Audio key response timeout");
                Err(AudioKeyError::Timeout.into())
            }
            Ok(k) => k?,
        }
    }

    fn send_key_request(&self, seq: u32, track: SpotifyId, file: FileId) -> Result<(), Error> {
        let mut data: Vec<u8> = Vec::new();
        data.write_all(&file.0)?;
        data.write_all(&track.to_raw())?;
        data.write_u32::<BigEndian>(seq)?;
        data.write_u16::<BigEndian>(0x0000)?;

        self.session().send_packet(PacketType::RequestKey, data)
    }
}
