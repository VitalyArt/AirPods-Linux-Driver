use crate::bluetooth::aacp::AACPManager;
use std::sync::Arc;

pub struct DeviceManagers {
    aacp: Option<Arc<AACPManager>>,
}

impl DeviceManagers {
    pub async fn shutdown(&self) {
        if let Some(aacp) = &self.aacp {
            aacp.shutdown().await;
        }
    }

    pub fn with_aacp(aacp: AACPManager) -> Self {
        Self {
            aacp: Some(Arc::new(aacp)),
        }
    }

    pub fn set_aacp(&mut self, manager: AACPManager) {
        self.aacp = Some(Arc::new(manager));
    }

    pub fn get_aacp(&self) -> Option<Arc<AACPManager>> {
        self.aacp.clone()
    }
}
