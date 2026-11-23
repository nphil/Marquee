//! Dolby TrueHD to the receiver, untouched, through a television whose pipeline has no TrueHD
//! input: the access units are packed into the MAT carrier ([`mat`]) and fed as 8-channel PCM that
//! the LumaTV helper routes past LG's mixer to the eARC output.
pub mod mat;
