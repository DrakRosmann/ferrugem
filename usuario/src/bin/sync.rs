//! sync — grava no disco tudo o que está esperando no cache.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    sys::sincronizar();
    0
}
