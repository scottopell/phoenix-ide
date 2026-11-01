#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx", "httpx-sse", "opentelemetry-proto==1.39.0"]
# ///
"""Single ordinary geometry reproduction. Artifact reuse, never qualification."""
import contextlib,json,os,sqlite3,tempfile,subprocess,hashlib,socket,signal
from pathlib import Path
from urllib.parse import urlencode,quote
import history_opening_browser as opening
def deadline(signum, frame):
 raise TimeoutError('bounded geometry lifetime exhausted')
signal.signal(signal.SIGTERM, deadline)
signal.signal(signal.SIGALRM, deadline)
signal.alarm(150)
s=opening.shipping
app=Path(os.environ['APP_ROOT']).resolve(); artifact=Path(os.environ['GEOMETRY_ARTIFACT']).resolve()
out=Path(os.environ['GEOMETRY_OUTPUT']).resolve();out.mkdir(parents=True,exist_ok=True)
receipt=json.loads((artifact/'opening-instrumented-receipt.json').read_text())
owner=json.loads((artifact/'opening-ui-build-owner.json').read_text())
assert receipt['app_source']==opening.PIN and receipt['app_tree']==opening.TREE_PIN
assert receipt['harness_source']=='98718c5c05b8191a1cfbf418652a1d08ec401c6c'
exe=artifact/'opening-instrumented-libtest';assert s.digest(exe)==receipt['executable_sha256'];exe.chmod(0o700)
with exe.open('rb') as stream: assert stream.read(20)[:4]==b'\x7fELF'
assert s.git(app,'rev-parse','HEAD')==opening.PIN and s.git(app,'rev-parse','HEAD^{tree}')==opening.TREE_PIN
manifest=artifact/'pinned-app/ui/controlled-functional-build.json'
binding=owner['ui_artifacts']['controlled_functional'];assert binding==receipt['ui_artifacts']['controlled_functional']
assert s.digest(manifest)==binding['manifest_sha256']
ui=artifact/'pinned-app/ui/dist-controlled-functional'
# upload-artifact omits dotfiles: restore only the independently attested empty placeholder.
assert binding['ui_files']['ui/dist-controlled-functional/.gitkeep']==hashlib.sha256(b'').hexdigest()
(ui/'.gitkeep').write_bytes(b'')
assert {str(p.relative_to(artifact/'pinned-app')):s.digest(p) for p in ui.rglob('*') if p.is_file()}==binding['ui_files']
assert s.digest(app/'ui/pnpm-lock.yaml')==receipt['ui_lock_sha256']
s.write_json(out/'provenance.json',{'scope':'diagnostic_not_acceptance','artifact':12084070461,'app_source':opening.PIN,'app_tree':opening.TREE_PIN,'executable_sha256':s.digest(exe),'controlled_manifest_sha256':s.digest(manifest),'fixture':'recreated_recipe_not_original_database'})
h=s.load_app_harness(app);failures=opening.LifecycleFailures()
with tempfile.TemporaryDirectory(prefix='geo-',dir='/tmp') as owned:
 work=Path(owned);work.chmod(0o700);control=work/'c.sock';port=h._free_port();base=f'http://127.0.0.1:{port}'
 env={'PATH':os.environ['PATH'],'HOME':str(work),'TMPDIR':str(work),'PHOENIX_PORT':str(port),'PHOENIX_TEST_OPENING_CONTROL':str(control)}
 telemetry=opening.load_telemetry_modules(app)
 with telemetry[0].telemetry_sink(out,'geometry-once',lifetime_seconds=180) as receiver:
  env=opening.telemetry_environment(env,receiver)
  with s.owned_process([str(exe),opening.DRIVER,'--exact','--ignored','--test-threads=1','--nocapture'],work,env,out/'private-runtime.log') as server:
   startup=None
   try:
    opening.readiness(server,base+'/api/auth/status');startup=opening.startup_identity(out/'private-runtime.log',exe,work,port)
    fixture=s.Fixture(h,base,out)
    try:
     opening.authenticate_fixture(fixture.client,opening.owned_qualification_password(startup,out/'private-credential-identity.json'))
     fixture.h=opening.AuthenticatedHarness(h,fixture.client);fixture.completion=opening.OwnedFixtureCompletion(startup,fixture)
     cwd=work/'cwd';cwd.mkdir();aggregate=opening.equal_text_fixture(fixture,fixture.build('ordinary',cwd),writable=True)
    finally:fixture.client.close()
    source=aggregate['members'][0];mid=aggregate['old_message']
    with contextlib.closing(sqlite3.connect(Path(startup['db']).as_uri()+'?mode=ro',uri=True)) as db:
     assert db.execute('SELECT count(*) FROM messages WHERE conversation_id=? AND message_id=?',(source,mid)).fetchone()[0]==1
    route=aggregate['route']+'?'+urlencode({'source_transcript':source})+'#message-'+quote(mid,safe='')
    input_path=work/'input.json';s.write_json(input_path,{'id':mid,'source':source,'route':route,'endpoint':aggregate['endpoint']})
    previewport=h._free_port();config=work/'preview.mjs';fault=work/'fault.json';s.write_json(fault,{'aggregatePath':aggregate['endpoint'],'failRoster':False,'streamPath':'','drop':0})
    script=Path(__file__).resolve().parents[2]/'ui/scripts/qualify-history-opening-realserver.mjs'
    config.write_text('import {previewTransport} from '+json.dumps(script.as_uri())+'; const c='+json.dumps({'root':str(app/'ui'),'build':{'outDir':str(ui)},'preview':{'host':'127.0.0.1','port':previewport,'strictPort':True}})+'; c.preview.proxy=previewTransport('+json.dumps(base)+','+json.dumps(str(fault))+','+json.dumps(str(work/'transport.jsonl'))+');export default c;')
    nodeenv=os.environ|{'HOME':str(work),'HISTORY_BASE_URL':f'http://127.0.0.1:{previewport}','HISTORY_AUTH_HOME':startup['home'],'HISTORY_AUTH_CREDENTIAL_FILE':startup['credential_file'],'GEOMETRY_INPUT':str(input_path)}
    with s.owned_process(['node',str(app/'ui/node_modules/vite/bin/vite.js'),'preview','--config',str(config)],work,nodeenv,out/'private-preview.log') as preview:
     opening.readiness(preview,nodeenv['HISTORY_BASE_URL'])
     completed=subprocess.run(['node',str(script.parent/'history-geometry-repro.mjs')],env=nodeenv,cwd=work,timeout=65)
     s.write_json(out/'driver-exit.json',{'returncode':completed.returncode,'acceptance':False})
   finally:
    signal.alarm(35)
    if startup:
     try:
      with socket.socket(socket.AF_UNIX) as client:
       client.settimeout(2);client.connect(str(control));client.sendall(b'{"command":"stop"}\n');assert client.recv(4096)
      server.wait(timeout=20)
      opening.normal_server_completion(server,out,startup,control,failures)
     except Exception:opening.forced_server_shutdown(server,out,control,failures);raise
signal.alarm(0)
s.write_json(out/'completion.json',{'scope':'diagnostic_not_acceptance','server':'normal','receiver':'closed','original_database':False})

if completed.returncode != 0:
 raise RuntimeError('geometry capture failed; see retained sanitized geometry and driver-exit')
