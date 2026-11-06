import sys
a,b=sys.argv[1:3]
def ler(p):
    r={}
    for l in open(p):
        t=l.split()
        if len(t)>=2: r[(t[-5] if len(t)>=5 else t[0]).split('>')[-1]]=t[-4:] if len(t)>=5 else None
    return r
A,B=ler(a),ler(b); ruim=0
for k,v in A.items():
    w=B.get(k)
    if v is None or w is None:
        if v!=w: print(k,v,w); ruim+=1
        continue
    if any(abs(int(x)-int(y))>1 for x,y in zip(v,w)): print(k,'chrome',' '.join(v),'| motor',' '.join(w)); ruim+=1
sys.exit(1 if ruim else 0)
