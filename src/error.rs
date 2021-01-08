/*
This file is part of jpegxl-rs.

jpegxl-rs is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

jpegxl-rs is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with jpegxl-rs.  If not, see <https://www.gnu.org/licenses/>.
*/

// #![allow(non_upper_case_globals)]
// #![allow(non_camel_case_types)]
// #![allow(non_snake_case)]

use jpegxl_sys::*;

/// Errors derived from JxlDecoderStatus
#[derive(Debug)]
pub enum JXLDecodeError {
    /// Cannot create a decoder
    CannotCreateDecoder,
    /// Unknown Error
    // TODO: underlying library is working on a way to retrieve error message
    GenericError,
    /// Need more input bytes
    NeedMoreInput,
    /// Unknown status
    UnknownStatus(JxlDecoderStatus),
}

/// Errors derived from JxlEncoderStatus
#[derive(Debug)]
pub enum JXLEncodeError {
    /// Unknown Error
    // TODO: underlying library is working on a way to retrieve error message
    GenericError,
    /// Need more input bytes
    NeedMoreOutput,
    /// Unknown status
    UnknownStatus(JxlEncoderStatus),
}

impl std::fmt::Display for JXLDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl std::error::Error for JXLDecodeError {}

/// Error mapping from underlying C const to JxlDecoderStatus enum
pub fn check_dec_status(status: JxlDecoderStatus) -> Result<(), JXLDecodeError> {
    match status {
        JxlDecoderStatus_JXL_DEC_SUCCESS => Ok(()),
        JxlDecoderStatus_JXL_DEC_ERROR => Err(JXLDecodeError::GenericError),
        JxlDecoderStatus_JXL_DEC_NEED_MORE_INPUT => Err(JXLDecodeError::NeedMoreInput),
        _ => Err(JXLDecodeError::UnknownStatus(status)),
    }
}

/// Error mapping from underlying C const to JxlEncoderStatus enum
pub fn check_enc_status(status: JxlEncoderStatus) -> Result<(), JXLEncodeError> {
    match status {
        JxlEncoderStatus_JXL_ENC_SUCCESS => Ok(()),
        JxlEncoderStatus_JXL_ENC_ERROR => Err(JXLEncodeError::GenericError),
        JxlEncoderStatus_JXL_ENC_NEED_MORE_OUTPUT => Err(JXLEncodeError::NeedMoreOutput),
        _ => Err(JXLEncodeError::UnknownStatus(status)),
    }
}