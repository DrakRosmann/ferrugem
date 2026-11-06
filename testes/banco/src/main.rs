extern crate alloc;
pub mod js;
pub mod mat;
pub mod css;
pub mod dom;
pub mod caixas;
pub mod html;
pub mod http;
pub mod web;
use js::vm::{Vm, Valor};
use std::time::{SystemTime, UNIX_EPOCH};
fn agora() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 }
struct M; 
impl caixas::Medidor for M {
    fn largura(&mut self, t:&str, tam:u16, _:bool, _:bool)->i32 { let c=t.chars().count() as i32; c*(tam as i32*9/14) }
    fn metricas(&mut self, tam:u16, _:bool)->(i32,i32){ let h=tam as i32*20/14; (h*3/4,h) }
}
struct H;
impl caixas::Hospede for H {
    fn imagem(&mut self, _:&str)->(usize,Option<(i32,i32)>){ (0,Some((100,50))) }
    fn controle(&mut self, _:u32)->(i32,i32){ (100,26) }
}
struct FH{d:dom::Dom,e:css::EstadoCss,campos:std::collections::BTreeMap<u32,(String,bool,i32)>,foco:Option<u32>,log:Vec<String>}
use std::rc::Rc;
impl web::Hospedeiro for FH{
 fn dom(&mut self)->&mut dom::Dom{&mut self.d}
 fn estado_css(&mut self)->&css::EstadoCss{&self.e}
 fn valor_campo(&mut self,n:u32)->Option<String>{ let nome=self.d.nome(n).to_string(); if !matches!(nome.as_str(),"input"|"select"|"textarea"){return None} if let Some(c)=self.campos.get(&n){ if nome=="select"{ let mut o=vec![]; self.d.todos(n,"option",&mut o); return o.get(c.2.max(0) as usize).map(|x|self.d.atributo(*x,"value").map(str::to_string).unwrap_or_else(||self.d.texto(*x))) } return Some(c.0.clone())} if nome=="select"{ let i=self.indice_selecionado(n); let mut o=vec![]; self.d.todos(n,"option",&mut o); return o.get(i.max(0) as usize).map(|x|self.d.atributo(*x,"value").map(str::to_string).unwrap_or_else(||self.d.texto(*x))) } if nome=="textarea"{return Some(self.d.texto(n))} let t=self.d.atributo(n,"type").unwrap_or(""); Some(self.d.atributo(n,"value").map(str::to_string).unwrap_or_else(|| if t=="checkbox"||t=="radio" {"on".into()} else {String::new()})) }
 fn definir_valor_campo(&mut self,n:u32,v:&str){ if self.d.e(n,"select"){ let mut o=vec![]; self.d.todos(n,"option",&mut o); if let Some(i)=o.iter().position(|x|self.d.atributo(*x,"value")==Some(v)){ let c=self.d.atributo(n,"checked").is_some(); self.campos.insert(n,(String::new(),c,i as i32)); } return } let m=self.marcado(n); let i=self.indice_selecionado(n); self.campos.insert(n,(v.to_string(),m,i)); }
 fn marcado(&mut self,n:u32)->bool{ self.campos.get(&n).map(|c|c.1).unwrap_or(self.d.atributo(n,"checked").is_some()) }
 fn definir_marcado(&mut self,n:u32,v:bool){ let t=self.valor_campo(n).unwrap_or_default(); let i=self.indice_selecionado(n); self.campos.insert(n,(t,v,i)); }
 fn indice_selecionado(&mut self,n:u32)->i32{ if let Some(c)=self.campos.get(&n){ if self.d.e(n,"select"){return c.2} } let mut o=vec![]; self.d.todos(n,"option",&mut o); o.iter().position(|x|self.d.atributo(*x,"selected").is_some()).unwrap_or(0) as i32 }
 fn definir_indice(&mut self,n:u32,i:i32){ self.campos.insert(n,(String::new(),false,i)); }
 fn retangulo(&mut self,_:u32)->Option<(i32,i32,i32,i32)>{Some((0,0,10,10))}
 fn estilo(&mut self,_:u32)->Option<Rc<css::Estilo>>{None}
 fn janela(&mut self)->(i32,i32){(800,600)}
 fn altura_documento(&mut self)->i32{600}
 fn rolagem(&mut self)->i32{0}
 fn rolar_para(&mut self,_:i32){}
 fn url(&mut self)->String{"http://exemplo.com/dir/p.html".into()}
 fn navegar(&mut self,_:&str,_:bool){} fn mudar_endereco(&mut self,_:&str,_:bool){} fn voltar(&mut self,_:i32){}
 fn focar(&mut self,n:Option<u32>){self.foco=n} fn foco(&mut self)->Option<u32>{self.foco}
 fn acao_clique(&mut self,_:u32){} fn enviar_formulario(&mut self,_:u32){} fn limpar_formulario(&mut self,_:u32){}
 fn pedir(&mut self,_:u32,_:&str,_:&str,_:&[(String,String)],_:Option<Vec<u8>>,_:bool){} fn cancelar_pedido(&mut self,_:u32){}
 fn pedir_imagem(&mut self,_:&str){} fn tamanho_imagem(&mut self,_:&str)->Option<(i32,i32)>{None}
 fn avisar(&mut self,_:&str){} fn console(&mut self,n:u8,t:&str){ eprintln!("[{}] {}",n,t); self.log.push(t.into()) }
 fn cookies(&mut self)->String{String::new()} fn definir_cookie(&mut self,_:&str){}
 fn ler_armazenamento(&mut self)->String{String::new()} fn gravar_armazenamento(&mut self,_:&str){} fn mudou_titulo(&mut self){}
}
fn rodar_dom(arq:&str){
 let t=std::fs::read_to_string(arq).unwrap();
 let d=dom::Dom::de_html(&t);
 let mut h=FH{d,e:css::EstadoCss::default(),campos:Default::default(),foco:None,log:vec![]};
 let mut w=web::Web::novo(&mut h,agora);
 let mut sc=vec![]; h.d.todos(0,"script",&mut sc);
 for n in sc { let f=h.d.texto(n); w.rodar_script(&mut h,n,&f); }
 w.marcar_estado(1); w.disparar(&mut h,0,"DOMContentLoaded",true,false,&[]);
 w.marcar_estado(2); w.disparar(&mut h,dom::NADA,"load",false,false,&[]);
 for _ in 0..30 { std::thread::sleep(std::time::Duration::from_millis(10)); w.passo(&mut h); }
 for n in w.tomar_scripts_novos(){ let f=h.d.texto(n); w.rodar_script(&mut h,n,&f); }
 w.rodar_script(&mut h,dom::NADA,"console.log(L.join(String.fromCharCode(10)))");
 println!("{}",h.log.last().cloned().unwrap_or_default());
}
fn layout(arq:&str, w:i32) {
    let f = std::fs::read_to_string(arq).unwrap();
    let d = dom::Dom::de_html(&f);
    let ua = css::Folha::ler(css::FOLHA_PADRAO);
    let mut folhas = vec![ua];
    let mut st = vec![]; d.todos(0, "style", &mut st);
    for s in st { folhas.push(css::Folha::ler(&d.texto(s))); }
    let refs: Vec<&css::Folha> = folhas.iter().collect();
    let mut est = css::EstadoCss::default(); est.largura=w; est.altura=600;
    let c = css::calcular(&d, &refs, &est);
    let (mut m, mut h) = (M,H);
    let base = |s:&str| Some(s.to_string());
    let motor = caixas::Motor::novo(&d,&c,&mut m,&mut h,&base,w,600);
    let l = motor.diagramar();
    let mut todos=vec![]; d.todos(0,"*",&mut todos);
    for n in todos { if let Some(id)=d.atributo(n,"id") { if let Some(r)=l.retangulos.get(&n) { println!("{} {} {} {} {}",id,r.0,r.1,r.2,r.3); } else { println!("{} -",id); } } }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args[1]=="--dom" { rodar_dom(&args[2]); return; }
    if args[1]=="--layout" { layout(&args[2], args[3].parse().unwrap()); return; }
    let fonte = std::fs::read_to_string(&args[1]).unwrap();
    let mut vm = Vm::novo();
    vm.ganchos.agora_ms = agora;
    vm.ganchos.console = |_, _, t| { println!("{}", t) };
    let r = vm.avaliar(&fonte);
    if let Err(e) = r { js::relatar_erro(&mut vm, e); }
    js::rodar_microtarefas(&mut vm);
    let _ = Valor::Undef;
}
