"""Real-corpus scaling test. Copies via hardlinks in an isolated test directory.
Each output is independently decoded, and the originals are hashed before and after.
Never changes or deletes the supplied corpus. Requires Pillow and psutil.
"""
import argparse, json, math, os, pathlib, platform, shutil, subprocess, time
from PIL import Image
from verify import recipe, run_job, sha

def main():
    p=argparse.ArgumentParser();p.add_argument('--exe',type=pathlib.Path,required=True);p.add_argument('--corpus',type=pathlib.Path,required=True);p.add_argument('--work',type=pathlib.Path,required=True);p.add_argument('--counts',type=int,nargs='+',default=[1000,10000]);p.add_argument('--workers',type=int,nargs='+',default=[1,2,4,6]);p.add_argument('--fit',action='store_true');p.add_argument('--memory',type=int,default=1536);a=p.parse_args()
    root=a.work.resolve();root.mkdir(parents=True,exist_ok=True);originals=sorted(f for f in a.corpus.resolve().iterdir() if f.suffix.lower() in ['.jpg','.jpeg']);assert originals,'No JPEG corpus found'
    before={str(f):sha(f)for f in originals};manifest=[]
    for f in originals:
        with Image.open(f)as im:manifest.append(dict(name=f.name,size=im.size,orientation=im.getexif().get(274,1),bytes=f.stat().st_size,camera=str(im.getexif().get(272)),sha256=before[str(f)]))
    (root/'corpus.json').write_text(json.dumps(manifest,indent=2))
    results=[]
    for count in a.counts:
        inputs=root/f'input-{count}';inputs.mkdir(exist_ok=True)
        seeds=root/f'seeds-{count}';seeds.mkdir(exist_ok=True)
        for i in range(count):
            source=originals[i%len(originals)];dest=inputs/f'{i:05d}-{source.name}'
            # NTFS permits only 1,024 hardlinks per file. Bound each seed group.
            seed=seeds/f'{i//(500*len(originals)):04d}-{source.name}'
            if not seed.exists():shutil.copy2(source,seed)
            if not dest.exists():os.link(seed,dest)
        for workers in a.workers:
            name=f'n{count}-w{workers}';r=recipe(inputs,root/name/'output',workers=workers,memory_mib=a.memory);r['size']['resize']=a.fit
            print(f'Starting {name}: {count} JPEGs, {workers} workers, resize={a.fit}',flush=True)
            result=run_job(a.exe.resolve(),r,root,name);result.update(machine=platform.platform(),corpus_unique=len(originals),corpus_count=count,corpus_method='Repeated unmodified JPEGs, hardlinked to isolated seed copies (500 links per seed)' ,worker_count=workers,photos_per_minute=result['final']['completed']*60/result['elapsed_seconds'])
            result['integrity']={'decoded_unique_contents':0,'hash_verified':0,'density_checked':0}
            decoded_hashes=set();repeated_source_hashes={};nondeterministic=[]
            outputs=list(pathlib.Path(r['destination']).glob('*.jpg'));assert len(outputs)==count,result
            for f in outputs:
                digest=sha(f)
                with Image.open(f)as im:
                    # Identical encoded bytes need one full decode; reopen every header.
                    if digest not in decoded_hashes:
                        im.load();decoded_hashes.add(digest);result['integrity']['decoded_unique_contents']+=1
                    assert im.info.get('dpi')==(300,300),(f,im.info.get('dpi'))
                    source_info=manifest[int(f.name.split('-')[0])%len(manifest)]
                    prior=repeated_source_hashes.setdefault(source_info['sha256'],digest)
                    if prior!=digest:nondeterministic.append(dict(file=str(f),sha256=digest,first_sha256=prior))
                    width,height=source_info['size']
                    if source_info['orientation']>=5:width,height=height,width
                    factor=min(1800/width,1200/height)if a.fit else 1
                    expected=(max(1,math.floor(width*factor+.5)),max(1,math.floor(height*factor+.5)))
                    assert im.size==expected,(f,im.size,expected)
                    assert im.getexif().get(274,1)==1 and not im.getexif().get(34853)
                    assert im.info.get('icc_profile')
                    result['integrity']['hash_verified']+=1;result['integrity']['density_checked']+=1
            assert before=={str(f):sha(f)for f in originals},'Originals changed'
            result['integrity']['nondeterministic_exports']=nondeterministic
            results.append(result);(root/'results.json').write_text(json.dumps(results,indent=2));print(f"Finished {name}: {result['photos_per_minute']:.2f} photos/minute, peak {result['peak_private_bytes']/1048576:.1f} MiB",flush=True)
            assert not nondeterministic, f'{len(nondeterministic)} repeated-source exports differ; see results.json'
    print('All jobs and independent exported-file checks passed.',flush=True)
if __name__=='__main__':main()
