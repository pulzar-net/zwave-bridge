//! NVM backup: the controller's whole memory image (network, nodes, routes),
//! enough to replace a dead stick without re-pairing.

use crate::controller::Controller;
use crate::Error;
use std::time::Duration;
use tracing::info;
use zwave_serial::message::{nvm, nvm_request, NvmResponse};
use zwave_serial::{DataFrame, FunctionId as F};

/// Largest image accepted (the biggest current controller NVM is well under this).
pub const MAX_NVM: usize = 4 * 1024 * 1024;
const CHUNK: u8 = 48;

impl Controller {
    /// Reads the controller's NVM image. Uses the extended or the 16-bit NVM
    /// operations on 700 and 800 series sticks and the external NVM
    /// functions on the 500 series. Normal operation pauses while it runs.
    pub async fn backup_nvm(&self) -> Result<Vec<u8>, Error> {
        let caps = &self.inner.caps;
        let image = if caps.supports(F::EXTENDED_NVM_OPERATIONS) {
            self.backup_700(true).await?
        } else if caps.supports(F::NVM_OPERATIONS) {
            self.backup_700(false).await?
        } else if caps.supports(F::EXT_NVM_READ_LONG_BUFFER) && caps.supports(F::GET_NVM_ID) {
            self.backup_500().await?
        } else {
            return Err(Error::Controller("the controller offers no NVM backup function".into()));
        };
        info!(bytes = image.len(), "NVM backup read");
        Ok(image)
    }

    async fn nvm_op(&self, extended: bool, op: u8, len: u8, offset: u32) -> Result<NvmResponse, Error> {
        let r = self.inner.api.request(nvm_request(extended, op, len, offset), Duration::from_secs(5)).await?;
        NvmResponse::parse(&r.payload, extended).map_err(|e| Error::Controller(e.to_string()))
    }

    async fn backup_700(&self, extended: bool) -> Result<Vec<u8>, Error> {
        let open = self.nvm_op(extended, nvm::OPEN, 0, 0).await?;
        if open.status != nvm::STATUS_OK {
            return Err(Error::Controller(format!("NVM open failed: {:#04x}", open.status)));
        }
        let size = open.offset_or_size as usize;
        let size = if size == 0 && !open.data.is_empty() {
            open.data.iter().fold(0usize, |a, &b| (a << 8) | usize::from(b))
        } else {
            size
        };
        if size == 0 || size > MAX_NVM {
            let _ = self.nvm_op(extended, nvm::CLOSE, 0, 0).await;
            return Err(Error::Controller(format!("NVM size {size} out of range")));
        }
        let mut image = Vec::with_capacity(size);
        let result = async {
            while image.len() < size {
                let want = CHUNK.min((size - image.len()).min(255) as u8);
                let r = self.nvm_op(extended, nvm::READ, want, image.len() as u32).await?;
                if r.status != nvm::STATUS_OK && r.status != nvm::STATUS_EOF {
                    return Err(Error::Controller(format!("NVM read failed: {:#04x}", r.status)));
                }
                if r.data.is_empty() {
                    break;
                }
                let take = r.data.len().min(size - image.len());
                image.extend_from_slice(&r.data[..take]);
                if r.status == nvm::STATUS_EOF {
                    break;
                }
            }
            Ok(())
        }
        .await;
        let close = self.nvm_op(extended, nvm::CLOSE, 0, 0).await;
        result?;
        close?;
        Ok(image)
    }

    async fn backup_500(&self) -> Result<Vec<u8>, Error> {
        let id = self.inner.api.request(DataFrame::request(F::GET_NVM_ID, vec![]), Duration::from_secs(2)).await?;
        let code = *id.payload.get(2).ok_or(Error::Controller("NVM id too short".into()))?;
        if !(0x0e..=0x16).contains(&code) {
            return Err(Error::Controller(format!("unknown NVM size code {code:#04x}")));
        }
        let size = 1usize << code;
        let mut image = Vec::with_capacity(size);
        while image.len() < size {
            let len = 64.min(size - image.len()) as u16;
            let off = image.len() as u32;
            let p = vec![(off >> 16) as u8, (off >> 8) as u8, off as u8, (len >> 8) as u8, len as u8];
            let r = self.inner.api.request(DataFrame::request(F::EXT_NVM_READ_LONG_BUFFER, p), Duration::from_secs(5)).await?;
            if r.payload.is_empty() {
                return Err(Error::Controller("empty NVM read".into()));
            }
            let take = r.payload.len().min(size - image.len());
            image.extend_from_slice(&r.payload[..take]);
        }
        Ok(image)
    }
}
