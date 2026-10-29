//! Relógio de tempo real (RTC) do chip CMOS: a data e a hora que a placa-mãe
//! mantém mesmo desligada (com a bateria). No QEMU, é a hora do computador (em UTC).
//!
//! Lemos uma vez na partida e, dali em diante, somamos o tempo do timer.

use crate::pit;
use crate::porta::{inb, outb};
use core::sync::atomic::{AtomicU64, Ordering};

static NA_PARTIDA: AtomicU64 = AtomicU64::new(0);

fn ler(registrador: u8) -> u8 {
    unsafe {
        outb(0x70, registrador);
        inb(0x71)
    }
}

/// Dias desde 1970-01-01 (algoritmo de Howard Hinnant, "days_from_civil").
fn dias_desde_1970(ano: i64, mes: i64, dia: i64) -> i64 {
    let ano = if mes <= 2 { ano - 1 } else { ano };
    let era = ano.div_euclid(400);
    let ano_da_era = ano - era * 400;
    let dia_do_ano = (153 * ((mes + 9) % 12) + 2) / 5 + dia - 1;
    let dia_da_era = ano_da_era * 365 + ano_da_era / 4 - ano_da_era / 100 + dia_do_ano;
    era * 146097 + dia_da_era - 719468
}

pub fn init() {
    while ler(0x0A) & 0x80 != 0 {} // espera o chip terminar de atualizar
    let mut valores = [ler(0x00), ler(0x02), ler(0x04), ler(0x07), ler(0x08), ler(0x09)];
    let formato = ler(0x0B);
    let pm = valores[2] & 0x80 != 0;
    valores[2] &= 0x7F;
    if formato & 0x04 == 0 {
        // Os números vêm em BCD: 0x59 quer dizer 59.
        for v in valores.iter_mut() {
            *v = (*v >> 4) * 10 + (*v & 0x0F);
        }
    }
    let [segundo, minuto, mut hora, dia, mes, ano] = valores.map(|v| v as i64);
    if formato & 0x02 == 0 && pm {
        hora = (hora + 12) % 24; // relógio de 12 horas
    }
    let agora = dias_desde_1970(2000 + ano, mes, dia) * 86400 + hora * 3600 + minuto * 60 + segundo;
    NA_PARTIDA.store(agora as u64, Ordering::Relaxed);
}

/// Segundos desde 1970-01-01 00:00 UTC (o "tempo Unix").
pub fn agora() -> u64 {
    NA_PARTIDA.load(Ordering::Relaxed) + pit::milissegundos() / 1000
}
