"""Sample the installed app and all WebView2 descendants; never drives the UI.
Start after scanning and before Process Photos. Stop automatically when the job
finishes, or create the --stop-file. Requires psutil. Journal reads are read-only.
"""
import argparse, datetime, json, pathlib, platform, sqlite3, time
import psutil

p=argparse.ArgumentParser()
p.add_argument('--pid',type=int,required=True)
p.add_argument('--journal',type=pathlib.Path,required=True)
p.add_argument('--output',type=pathlib.Path,required=True)
p.add_argument('--stop-file',type=pathlib.Path)
a=p.parse_args()
root=psutil.Process(a.pid);start=time.perf_counter();peak=0;peak_working=0;seen={};samples=[];running_at=None;done_at=None;last_state=None
connection=sqlite3.connect(a.journal.resolve().as_uri()+'?mode=ro',uri=True)
recipe=json.loads(connection.execute("SELECT value FROM meta WHERE key='recipe'").fetchone()[0])
machine={'os':platform.platform(),'cpu_logical':psutil.cpu_count(),'ram_bytes':psutil.virtual_memory().total,'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat()}
print('Monitoring installed app process tree; start the batch in the window.',flush=True)
while root.is_running():
    now=time.perf_counter();private=0;working=0
    try: processes=[root,*root.children(recursive=True)]
    except psutil.Error:break
    for proc in processes:
        try:
            info=proc.memory_info();private+=getattr(info,'private',info.rss);working+=info.rss;seen[proc.pid]=proc.name()
        except psutil.Error:pass
    peak=max(peak,private);peak_working=max(peak_working,working)
    if not samples or now-start-samples[-1]['seconds']>=.25:
        state=connection.execute("SELECT value FROM meta WHERE key='state'").fetchone()[0]
        counts=dict(connection.execute('SELECT status,count(*) FROM files GROUP BY status'))
        if state=='running'and running_at is None:
            running_at=now;recipe=json.loads(connection.execute("SELECT value FROM meta WHERE key='recipe'").fetchone()[0])
        samples.append(dict(seconds=round(now-start,3),private_bytes=private,state=state,counts=counts,processes=len(processes)))
        if state!=last_state:print(state,counts,flush=True);last_state=state
        if running_at and state in ['complete','cancelled','error']:
            done_at=now;break
    if a.stop_file and a.stop_file.exists():break
    time.sleep(.05)
result=dict(machine=machine,exe=root.exe()if root.is_running()else None,recipe=recipe,processes=seen,peak_private_bytes=peak,peak_working_set_bytes=peak_working,elapsed_seconds=None if not done_at else done_at-running_at,samples=samples)
a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(json.dumps(result,indent=2))
print(json.dumps({k:v for k,v in result.items()if k not in ['samples','recipe','processes']}),flush=True)
