//! Shared field-size token adapter.

use super::*;

pub(crate) type ComboSizeTokens = crate::tokens::FieldSizeTokens;

impl TextFieldSize {
    pub(crate) fn combo_tokens(self) -> ComboSizeTokens {
        self.tokens()
    }
}
