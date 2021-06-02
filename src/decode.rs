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

//! Decoder of JPEG XL format

#[allow(clippy::wildcard_imports)]
use jpegxl_sys::*;
use std::{
    mem::{ManuallyDrop, MaybeUninit},
    ptr::null,
};

use crate::{
    common::{Endianness, PixelType},
    errors::{check_dec_status, DecodeError},
    memory::JxlMemoryManager,
    parallel::JxlParallelRunner,
};

/// Basic Information
pub type BasicInfo = JxlBasicInfo;

/// Result of decoding
pub struct DecoderResult<T: PixelType> {
    /// Extra info
    pub info: ResultInfo,
    /// Decoded image data
    pub data: Vec<T>,
}

/// Extra info of the result
pub struct ResultInfo {
    /// Width of the image
    pub width: u32,
    /// Height of the image
    pub height: u32,
    /// Orientation
    pub orientation: JxlOrientation,
    /// Number of color channels per pixel
    pub num_channels: u32,
    /// ICC color profile
    pub icc_profile: Vec<u8>,
}

/// JPEG XL Decoder
#[derive(Builder)]
#[builder(build_fn(skip))]
#[builder(setter(strip_option))]
pub struct JxlDecoder<'prl, 'mm> {
    /// Opaque pointer to the underlying decoder
    #[builder(setter(skip))]
    dec: *mut jpegxl_sys::JxlDecoder,

    /// Number of channels for returned result
    ///
    /// Default: 4 for RGBA
    pub num_channels: u32,
    /// Endianness for returned result
    ///
    /// Default: native endian
    pub endianness: Endianness,
    /// Set pixel scanlines alignment for returned result
    ///
    /// Default: 0
    pub align: usize,

    /// Keep orientation or not
    ///
    /// Default: false, so the decoder rotates the image for you
    pub keep_orientation: bool,
    /// Set initial buffer for JPEG reconstruction.
    /// Larger one could be faster with fewer allocations
    ///
    /// Default: 1 KiB
    pub init_jpeg_buffer: usize,

    /// Parallel runner
    pub parallel_runner: Option<&'prl dyn JxlParallelRunner>,

    /// Store memory manager ref so it pins until the end of the decoder
    #[builder(setter(skip))]
    _memory_manager: Option<&'mm dyn JxlMemoryManager>,
}

impl<'prl, 'mm> JxlDecoderBuilder<'prl, 'mm> {
    fn _build(
        &self,
        memory_manager: Option<&'mm dyn JxlMemoryManager>,
    ) -> Result<JxlDecoder<'prl, 'mm>, DecodeError> {
        let dec = unsafe {
            memory_manager.map_or_else(
                || JxlDecoderCreate(null()),
                |mm| JxlDecoderCreate(&mm.manager()),
            )
        };

        if dec.is_null() {
            return Err(DecodeError::CannotCreateDecoder);
        }

        Ok(JxlDecoder {
            dec,
            num_channels: self.num_channels.unwrap_or(4),
            endianness: self.endianness.unwrap_or(Endianness::Native),
            align: self.align.unwrap_or(0),
            keep_orientation: self.keep_orientation.unwrap_or(false),
            init_jpeg_buffer: self.init_jpeg_buffer.unwrap_or(1024),
            parallel_runner: self.parallel_runner.flatten(),
            _memory_manager: memory_manager,
        })
    }

    /// Build a [`JxlDecoder`]
    ///
    /// # Errors
    /// Return [`DecodeError::CannotCreateDecoder`] if it fails to create the decoder.
    pub fn build(&self) -> Result<JxlDecoder<'prl, 'mm>, DecodeError> {
        Self::_build(self, None)
    }

    /// Build a [`JxlDecoder`] with custom memory manager
    ///
    /// # Errors
    /// Return [`DecodeError::CannotCreateDecoder`] if it fails to create the decoder.
    pub fn build_with(
        &self,
        mm: &'mm dyn JxlMemoryManager,
    ) -> Result<JxlDecoder<'prl, 'mm>, DecodeError> {
        Self::_build(self, Some(mm))
    }
}

union Data<T> {
    pixels: ManuallyDrop<Vec<T>>,
    jpeg: ManuallyDrop<Vec<u8>>,
}

impl<'prl, 'mm> JxlDecoder<'prl, 'mm> {
    fn decode_internal<T: PixelType>(
        &self,
        data: &[u8],
        reconstruct_jpeg: bool,
    ) -> Result<(ResultInfo, Data<T>), DecodeError> {
        let mut basic_info = MaybeUninit::uninit();
        let mut pixel_format = MaybeUninit::uninit();

        let mut icc_profile = vec![];
        let mut buffer = vec![];
        let mut jpeg_buffer = vec![];

        let mut jpeg_reconstructed = false;

        if reconstruct_jpeg {
            jpeg_buffer = vec![0; self.init_jpeg_buffer];
        }

        self.setup_decoder(reconstruct_jpeg)?;

        let next_in = data.as_ptr();
        let avail_in = std::mem::size_of_val(data) as _;

        check_dec_status(
            unsafe { JxlDecoderSetInput(self.dec, next_in, avail_in) },
            "Set input",
        )?;

        let mut status;
        loop {
            use JxlDecoderStatus::*;

            status = unsafe { JxlDecoderProcessInput(self.dec) };

            match status {
                Error => return Err(DecodeError::GenericError("Process input")),

                // Get the basic info
                BasicInfo => {
                    self.get_basic_info::<T>(basic_info.as_mut_ptr(), pixel_format.as_mut_ptr())?;
                }

                // Get color encoding
                ColorEncoding => {
                    icc_profile = self.get_icc_profile(unsafe { &*pixel_format.as_ptr() })?
                }

                // Get JPEG reconstruction buffer
                JpegReconstruction => {
                    jpeg_reconstructed = true;

                    check_dec_status(
                        unsafe {
                            JxlDecoderSetJPEGBuffer(
                                self.dec,
                                jpeg_buffer.as_mut_ptr(),
                                jpeg_buffer.len(),
                            )
                        },
                        "In JPEG reconstruction event",
                    )?;
                }

                // JPEG buffer need more space
                JpegNeedMoreOutput => {
                    let need_to_write = unsafe { JxlDecoderReleaseJPEGBuffer(self.dec) };

                    let old_len = jpeg_buffer.len();
                    jpeg_buffer.resize(old_len + need_to_write, 0);
                    check_dec_status(
                        unsafe {
                            JxlDecoderSetJPEGBuffer(
                                self.dec,
                                jpeg_buffer.as_mut_ptr(),
                                jpeg_buffer.len(),
                            )
                        },
                        "In JPEG need more output event, set without releasing",
                    )?;
                }

                // Get the output buffer
                NeedImageOutBuffer => {
                    buffer = self.output(unsafe { &*pixel_format.as_ptr() })?;
                }

                FullImage => continue,
                Success => {
                    if reconstruct_jpeg {
                        if !jpeg_reconstructed {
                            return Err(DecodeError::CannotReconstruct);
                        }

                        let remaining = unsafe { JxlDecoderReleaseJPEGBuffer(self.dec) };

                        jpeg_buffer.truncate(jpeg_buffer.len() - remaining);
                        jpeg_buffer.shrink_to_fit();
                    }

                    unsafe { JxlDecoderReset(self.dec) };

                    let info = unsafe { basic_info.assume_init() };
                    return Ok((
                        ResultInfo {
                            width: info.xsize,
                            height: info.ysize,
                            orientation: info.orientation,
                            num_channels: unsafe { pixel_format.assume_init().num_channels },
                            icc_profile,
                        },
                        if reconstruct_jpeg {
                            Data {
                                jpeg: ManuallyDrop::new(jpeg_buffer),
                            }
                        } else {
                            Data {
                                pixels: ManuallyDrop::new(buffer),
                            }
                        },
                    ));
                }
                _ => return Err(DecodeError::UnknownStatus(status)),
            }
        }
    }

    fn setup_decoder(&self, reconstruct_jpeg: bool) -> Result<(), DecodeError> {
        if let Some(runner) = self.parallel_runner {
            check_dec_status(
                unsafe {
                    JxlDecoderSetParallelRunner(self.dec, runner.runner(), runner.as_opaque_ptr())
                },
                "Set parallel runner",
            )?
        }

        let events = {
            use JxlDecoderStatus::*;

            let mut events = jxl_dec_events!(BasicInfo, ColorEncoding, FullImage);

            if reconstruct_jpeg {
                events |= JpegReconstruction as i32;
            }

            events
        };
        check_dec_status(
            unsafe { JxlDecoderSubscribeEvents(self.dec, events) },
            "Subscribe events",
        )?;

        check_dec_status(
            unsafe { JxlDecoderSetKeepOrientation(self.dec, self.keep_orientation) },
            "Set if keep orientation",
        )?;

        Ok(())
    }

    fn get_basic_info<T: PixelType>(
        &self,
        basic_info: *mut JxlBasicInfo,
        pixel_format: *mut JxlPixelFormat,
    ) -> Result<(), DecodeError> {
        unsafe {
            check_dec_status(
                JxlDecoderGetBasicInfo(self.dec, basic_info),
                "Get basic info",
            )?;
        }

        unsafe {
            *pixel_format = JxlPixelFormat {
                num_channels: self.num_channels,
                data_type: T::pixel_type(),
                endianness: self.endianness,
                align: self.align,
            };
        }

        Ok(())
    }

    fn get_icc_profile(&self, format: &JxlPixelFormat) -> Result<Vec<u8>, DecodeError> {
        let mut icc_size = 0;

        check_dec_status(
            unsafe {
                JxlDecoderGetICCProfileSize(
                    self.dec,
                    format,
                    JxlColorProfileTarget::Data,
                    &mut icc_size,
                )
            },
            "Get ICC profile size",
        )?;

        let mut icc_profile = vec![0; icc_size];

        check_dec_status(
            unsafe {
                JxlDecoderGetColorAsICCProfile(
                    self.dec,
                    format,
                    JxlColorProfileTarget::Data,
                    icc_profile.as_mut_ptr(),
                    icc_size,
                )
            },
            "Get ICC profile",
        )?;

        icc_profile.shrink_to_fit();

        Ok(icc_profile)
    }

    fn output<T: PixelType>(&self, pixel_format: &JxlPixelFormat) -> Result<Vec<T>, DecodeError> {
        let mut size = 0;
        check_dec_status(
            unsafe { JxlDecoderImageOutBufferSize(self.dec, pixel_format, &mut size) },
            "Get output buffer size",
        )?;

        let mut buffer = vec![T::default(); size];
        check_dec_status(
            unsafe {
                JxlDecoderSetImageOutBuffer(
                    self.dec,
                    pixel_format,
                    buffer.as_mut_ptr().cast(),
                    size,
                )
            },
            "Set output buffer",
        )?;

        buffer.shrink_to_fit();

        Ok(buffer)
    }

    /// Decode a JPEG XL image
    ///
    /// Currently only support RGB(A)8/16/32 encoded static image. Other info are discarded.
    /// # Errors
    /// Return a [`DecodeError`] when internal decoder fails
    pub fn decode<T: PixelType>(&self, data: &[u8]) -> Result<DecoderResult<T>, DecodeError> {
        let (info, data) = self.decode_internal(data, false)?;
        Ok(DecoderResult {
            info,
            data: unsafe { ManuallyDrop::into_inner(data.pixels) },
        })
    }

    /// Decode a JPEG XL image and reconstruct JPEG data
    ///
    /// Currently only support RGB(A)8/16/32 encoded static image. Other info are discarded.
    /// # Errors
    /// Return a [`DecodeError`] when internal decoder fails
    pub fn decode_jpeg(&self, data: &[u8]) -> Result<DecoderResult<u8>, DecodeError> {
        let (info, data) = self.decode_internal::<u8>(data, true)?;
        Ok(DecoderResult {
            info,
            data: unsafe { ManuallyDrop::into_inner(data.jpeg) },
        })
    }
}

impl<'prl, 'mm> Drop for JxlDecoder<'prl, 'mm> {
    fn drop(&mut self) {
        unsafe { JxlDecoderDestroy(self.dec) };
    }
}

/// Return a [`JxlDecoderBuilder`] with default settings
#[must_use]
pub fn decoder_builder<'prl, 'mm>() -> JxlDecoderBuilder<'prl, 'mm> {
    JxlDecoderBuilder::default()
}
