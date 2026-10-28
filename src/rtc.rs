//! Relógio de tempo real (RTC) do CMOS: a data e a hora de parede.
//!
//! Lido uma vez no boot; depois, a hora atual é a do boot mais o tempo
//! contado pelo timer. O RTC guarda a hora em UTC (é o que o QEMU usa).

use core::sync::atomic::{AtomicU64, Ordering};

use crate::cpu::{inb, outb};
use crate::timer;

static BOOT_EPOCH: AtomicU64 = AtomicU64::new(0);

fn read_register(register: u8) -> u8 {
    unsafe {
        outb(0x70, register | 0x80); // o bit 7 mantém a NMI desligada
        inb(0x71)
    }
}

fn updating() -> bool {
    read_register(0x0A) & 0x80 != 0
}

/// (segundos, minutos, horas, dia, mês, ano, século), crus.
fn snapshot() -> [u8; 7] {
    while updating() {}
    [0x00, 0x02, 0x04, 0x07, 0x08, 0x09, 0x32].map(read_register)
}

/// Dias desde 1970-01-01 (algoritmo de Howard Hinnant, `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn init() {
    // Lê até duas leituras seguidas darem o mesmo resultado (o RTC pode
    // virar o segundo no meio da leitura).
    let mut raw = snapshot();
    loop {
        let again = snapshot();
        if again == raw {
            break;
        }
        raw = again;
    }
    let status_b = read_register(0x0B);
    let binary = status_b & 0x04 != 0;
    let hours_24 = status_b & 0x02 != 0;
    let decode = |value: u8| if binary { value } else { (value & 0x0F) + (value >> 4) * 10 };

    let [seconds, minutes, hours, day, month, year, century] = raw;
    let pm = hours & 0x80 != 0;
    let mut hours = decode(hours & 0x7F) as i64;
    if !hours_24 {
        hours %= 12;
        if pm {
            hours += 12;
        }
    }
    let century = match decode(century) {
        c @ 19..=21 => c as i64,
        _ => 20,
    };
    let year = century * 100 + decode(year) as i64;
    let days = days_from_civil(year, decode(month) as i64, decode(day) as i64);
    let epoch = days * 86_400 + hours * 3600 + decode(minutes) as i64 * 60 + decode(seconds) as i64;
    BOOT_EPOCH.store((epoch.max(0) as u64).saturating_sub(timer::uptime_ms() / 1000), Ordering::Relaxed);
}

/// Segundos desde 1970-01-01 00:00 UTC.
pub fn now() -> u64 {
    BOOT_EPOCH.load(Ordering::Relaxed) + timer::uptime_ms() / 1000
}

/// Hora atual em milissegundos desde 1970 (para `clock_gettime`).
pub fn now_ms() -> u64 {
    BOOT_EPOCH.load(Ordering::Relaxed) * 1000 + timer::uptime_ms()
}
