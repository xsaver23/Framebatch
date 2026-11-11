"""Diagnostic only: independent Pillow/libjpeg repeated raw RGB hash test."""
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from PIL import Image,features
import argparse,hashlib,json,time,threading,psutil,platform

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--corpus',type=Path,required=True)
parser.add_argument('--work',type=Path,required=True)
parser.add_argument('--count',type=int,default=10000)
parser.add_argument('--workers',type=int,default=6)
args=parser.parse_args()
if args.count<1 or not 1<=args.workers<=16:parser.error('Positive count and 1 to 16 workers required')
files=sorted(p for p in args.corpus.resolve().iterdir() if p.is_file() and p.suffix.lower() in ['.jpg','.jpeg'])
if not files:parser.error('No JPEG files found')
dest=args.work.resolve()
dest.mkdir(parents=True,exist_ok=False)
lock=threading.Lock();results=[];completed=0

def decode(path):
    with Image.open(path) as im:
        im.load()
        raw=im.convert('RGB').tobytes()
    return hashlib.sha256(raw).hexdigest(),raw

baseline={str(p):decode(p)[0]for p in files}
source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest()for p in files}
start=time.perf_counter();peak=0;stop=threading.Event()
def monitor():
    global peak
    proc=psutil.Process()
    while not stop.wait(.05):peak=max(peak,proc.memory_info().private)
thread=threading.Thread(target=monitor);thread.start()
def worker(worker_id):
    global completed
    for i in range(worker_id,args.count,args.workers):
        path=files[i%len(files)]
        digest,raw=decode(path)
        if digest!=baseline[str(path)]:
            (dest/f'{i:05}-{path.stem}.rgb').write_bytes(raw)
            with lock:results.append(dict(index=i,source=str(path),expected=baseline[str(path)],actual=digest))
        del raw
        with lock:
            completed+=1
            if completed%500==0:print(json.dumps(dict(completed=completed,mismatches=len(results),elapsed_seconds=time.perf_counter()-start)),flush=True)
try:
    with ThreadPoolExecutor(max_workers=args.workers)as pool:list(pool.map(worker,range(args.workers)))
finally:
    stop.set();thread.join()
elapsed=time.perf_counter()-start
unchanged=all(hashlib.sha256(Path(p).read_bytes()).hexdigest()==v for p,v in source_hashes.items())
report=dict(diagnostic='Independent Pillow/libjpeg repeated full decode to RGB, SHA256 compared to first decode of each source',pillow=Image.__version__,libjpeg_turbo=features.version_feature('libjpeg_turbo'),platform=platform.platform(),workers=args.workers,requested_count=args.count,source_count=len(files),completed=completed,elapsed_seconds=elapsed,peak_private_bytes=peak,source_hashes_unchanged=unchanged,reference_hashes=baseline,mismatches=results,limits='Different executable and codec from Framebatch; a clean diagnostic does not clear either application or hardware. Not an application throughput benchmark.')
(dest/'result.json').write_text(json.dumps(report,indent=2))
print(json.dumps(report),flush=True)
assert unchanged and not results
