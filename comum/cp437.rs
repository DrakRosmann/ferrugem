//! A fonte de 256 caracteres da VGA (code page 437) e as letras que faltam nela.
//!
//! A fonte que a placa VGA traz é a do IBM PC de 1981: tem "á", "é", "ç",
//! mas não tem "ã", "õ" nem quase nenhuma maiúscula acentuada. Aqui geramos
//! as que o português precisa, no próprio estilo da fonte:
//!
//! - o *acento* sai da diferença entre dois glifos que já existem:
//!   `á` − `a` é o agudo, `ñ` − `n` é o til, `â` − `a` o circunflexo...
//! - nas *maiúsculas* não há espaço em cima: a letra é espremida (tiramos
//!   linhas repetidas, como as das hastes verticais) e o acento vai no alto,
//!   como a própria fonte faz no "É" e no "Ñ".
//!
//! As letras novas ocupam lugares de símbolos pouco usados (¥, ₧, ƒ, ⌐ e
//! algumas letras gregas). Este arquivo é usado pelo kernel (que gera a
//! fonte no boot e a usa no console) e pela ulib (que desenha texto na
//! interface gráfica com a mesma fonte, lida de /dev/fonte).

/// O acento de cada letra gerada: (base em CP437, glifo com o acento, glifo sem).
#[derive(Clone, Copy)]
enum Acento {
    Agudo,
    Grave,
    Circunflexo,
    Til,
}

impl Acento {
    /// De onde tirar o desenho do acento: (com acento, sem acento).
    fn exemplo(self) -> (u8, u8) {
        match self {
            Acento::Agudo => (0xA0, b'a'),       // á
            Acento::Grave => (0x85, b'a'),       // à
            Acento::Circunflexo => (0x83, b'a'), // â
            Acento::Til => (0xA4, b'n'),         // ñ
        }
    }
}

/// As letras geradas: (caractere, posição na fonte, letra-base, acento).
const GERADAS: [(char, u8, u8, Acento); 12] = [
    ('ã', 0x9D, b'a', Acento::Til),
    ('õ', 0x9E, b'o', Acento::Til),
    ('Á', 0x9F, b'A', Acento::Agudo),
    ('À', 0xA9, b'A', Acento::Grave),
    ('Â', 0xE0, b'A', Acento::Circunflexo),
    ('Ã', 0xE2, b'A', Acento::Til),
    ('Ê', 0xE3, b'E', Acento::Circunflexo),
    ('Í', 0xE4, b'I', Acento::Agudo),
    ('Ó', 0xE5, b'O', Acento::Agudo),
    ('Ô', 0xE7, b'O', Acento::Circunflexo),
    ('Õ', 0xE8, b'O', Acento::Til),
    ('Ú', 0xE9, b'U', Acento::Agudo),
];

/// Gera as letras acentuadas que faltam na fonte (256 glifos de 16 linhas).
pub fn completar(fonte: &mut [u8]) {
    if fonte.len() < 4096 {
        return;
    }
    let glifo = |f: &[u8], c: u8| -> [u8; 16] { f[c as usize * 16..c as usize * 16 + 16].try_into().unwrap() };
    for (_, posicao, base, acento) in GERADAS {
        let (com, sem) = acento.exemplo();
        let (com, sem) = (glifo(fonte, com), glifo(fonte, sem));
        // O acento: o que o glifo acentuado tem a mais, nas linhas de cima.
        let mut marca = [0u8; 5];
        for i in 0..5 {
            marca[i] = com[i] & !sem[i];
        }
        let primeira = marca.iter().position(|&b| b != 0).unwrap_or(0);
        let ultima = marca.iter().rposition(|&b| b != 0).unwrap_or(0);
        let linhas = &marca[primeira..=ultima];
        let letra = glifo(fonte, base);
        let mut novo = [0u8; 16];
        if base.is_ascii_lowercase() {
            // Minúscula: a letra fica onde está e o acento entra por cima, no
            // mesmo lugar do exemplo.
            novo = letra;
            for i in 0..5 {
                novo[i] |= marca[i];
            }
        } else {
            // Maiúscula (linhas 2 a 11): espreme numa linha a menos (3 a 11),
            // e o acento ocupa o alto (linhas 0 a 2), como o "Ñ" da fonte.
            let comeco = 3;
            let mut corpo = [0u8; 10];
            corpo.copy_from_slice(&letra[2..12]);
            let mut n = corpo.len();
            while n > 12 - comeco {
                // Tira uma linha igual à de cima (de preferência no meio).
                let meio = n / 2;
                let i = (1..n)
                    .filter(|&i| corpo[i] == corpo[i - 1])
                    .min_by_key(|&i| (i as isize - meio as isize).abs())
                    .unwrap_or(meio);
                corpo.copy_within(i + 1..n, i);
                n -= 1;
            }
            novo[..linhas.len()].copy_from_slice(linhas);
            novo[comeco..comeco + n].copy_from_slice(&corpo[..n]);
        }
        fonte[posicao as usize * 16..posicao as usize * 16 + 16].copy_from_slice(&novo);
    }
}

/// O número de um caractere Unicode na fonte (CP437 + as letras geradas).
/// O que não existe vira um quadradinho.
pub fn codigo(c: char) -> u8 {
    if let Some(&(_, posicao, _, _)) = GERADAS.iter().find(|g| g.0 == c) {
        return posicao;
    }
    match c {
        ' '..='~' => c as u8,
        'Ç' => 0x80, 'ü' => 0x81, 'é' => 0x82, 'â' => 0x83, 'ä' => 0x84,
        'à' => 0x85, 'å' => 0x86, 'ç' => 0x87, 'ê' => 0x88, 'ë' => 0x89,
        'è' => 0x8A, 'ï' => 0x8B, 'î' => 0x8C, 'ì' => 0x8D, 'Ä' => 0x8E,
        'Å' => 0x8F, 'É' => 0x90, 'ô' => 0x93, 'ö' => 0x94, 'ò' => 0x95,
        'û' => 0x96, 'ù' => 0x97, 'ÿ' => 0x98, 'Ö' => 0x99, 'Ü' => 0x9A,
        '¢' => 0x9B, '£' => 0x9C, 'á' => 0xA0, 'í' => 0xA1, 'ó' => 0xA2,
        'ú' => 0xA3, 'ñ' => 0xA4, 'Ñ' => 0xA5, 'ª' => 0xA6, 'º' => 0xA7,
        '¿' => 0xA8, '¬' => 0xAA, '½' => 0xAB, '¼' => 0xAC, '¡' => 0xAD,
        '«' => 0xAE, '»' => 0xAF, 'ß' => 0xE1, 'µ' => 0xE6, '°' => 0xF8,
        '±' => 0xF1, '÷' => 0xF6, '²' => 0xFD,
        // sem desenho próprio: a letra sem acento
        'È' | 'Ë' => b'E', 'Ì' | 'Î' | 'Ï' => b'I', 'Ò' => b'O', 'Ù' | 'Û' => b'U',
        '´' => b'\'', '¨' => b'"', '—' | '–' => b'-', '“' | '”' => b'"', '‘' | '’' => b'\'',
        '░' => 0xB0, '▒' => 0xB1, '▓' => 0xB2, '│' => 0xB3, '┤' => 0xB4,
        '║' => 0xBA, '╗' => 0xBB, '╝' => 0xBC, '┐' => 0xBF, '└' => 0xC0,
        '┴' => 0xC1, '┬' => 0xC2, '├' => 0xC3, '─' => 0xC4, '┼' => 0xC5,
        '╚' => 0xC8, '╔' => 0xC9, '═' => 0xCD, '┘' => 0xD9, '┌' => 0xDA,
        '█' => 0xDB, '▄' => 0xDC, '▀' => 0xDF, '·' => 0xFA, '■' => 0xFE,
        '→' => 0x1A, '←' => 0x1B, '↑' => 0x18, '↓' => 0x19, '•' => 0x07,
        '\n' => b'\n',
        '\u{8}' => 0x08,
        _ => 0xFE,
    }
}
