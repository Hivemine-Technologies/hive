use crate::error::Result;

pub fn run_init() -> Result<()> {
    super::wizard::run_wizard(None)
}
