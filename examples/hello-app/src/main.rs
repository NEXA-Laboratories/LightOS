#![no_std]
#![no_main]

use liblightos::*;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println!("Hello from LightOS app!");
    let pid = sys_getpid();
    println!("My PID: {}", pid);
    sys_exit(0);
}
