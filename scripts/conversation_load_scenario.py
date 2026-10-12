#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Generate arguments for the existing browser_profile.run_scenario tool.

This generates configuration only; it does not run a new benchmark harness.
"""
import argparse
import json
from pathlib import Path
import sqlite3


def scenario(fixture, url):
    manifest = json.loads((fixture / "manifest.json").read_text())
    target = manifest["target"]
    member = target["latest_member"]
    db = sqlite3.connect((fixture / "fixture.db").resolve().as_uri() + "?mode=ro", uri=True)
    try:
        message, sequence = db.execute("SELECT message_id,sequence_id FROM messages WHERE conversation_id=? ORDER BY sequence_id DESC LIMIT 1", (member,)).fetchone()
    finally:
        db.close()
    selector = 'button[title=' + json.dumps('Open conversation "' + target["title"] + '"') + ']'
    config = json.dumps({"route": "/c/" + target["product_id"], "lastId": "message-" + message, "sequence": sequence})
    install = """(() => {
      const config = CONFIG;
      const p = window.__loadBenchmark = {config, start: performance.now(), resources: []};
      const ready = () => {
        const transcript = document.querySelector('.message-virtual-transcript');
        const tail = document.getElementById(config.lastId);
        const observation = document.querySelector('[data-testid="persisted-conversation-owner"]');
        const input = document.querySelector('textarea');
        if (!transcript || transcript.clientHeight <= 0 || !tail || !observation || input || location.pathname !== config.route) return;
        if (!observation.textContent.includes('live connection unavailable') || !observation.textContent.includes('User input unavailable')) return;
        const r = tail.getBoundingClientRect();
        if (!tail.innerText.trim() || r.height <= 0 || r.bottom <= 0 || r.top >= innerHeight) return;
        if (!p.first) { p.first = performance.now(); requestAnimationFrame(() => requestAnimationFrame(() => {
          const gap = transcript.scrollHeight - transcript.clientHeight - transcript.scrollTop;
          const currentTail = document.getElementById(config.lastId);
          const currentRect = currentTail?.getBoundingClientRect();
          if (Math.abs(gap) <= 2 && transcript.clientHeight > 0 && currentRect?.height > 0 && currentRect.bottom > 0 && currentRect.top < innerHeight) {
            p.frameProxy = performance.now(); p.bottomGap = gap;
          } else { p.first = null; }
        })); }
      };
      p.domObserver = new MutationObserver(ready);
      p.domObserver.observe(document.body, {childList:true,subtree:true,attributes:true});
      p.resourceObserver = new PerformanceObserver(list => {
        for (const e of list.getEntries()) {
          const u = new URL(e.name);
          if (!u.pathname.startsWith('/api/')) continue;
          p.resources.push({path:u.pathname,query:Object.fromEntries([...u.searchParams].filter(([k])=>['message_limit','before','open_id'].includes(k))),
            start_ms:e.startTime-p.start,before_request_ms:e.requestStart-e.startTime,
            ttfb_ms:e.responseStart-e.requestStart,download_ms:e.responseEnd-e.responseStart,
            duration_ms:e.duration,status:e.responseStatus,decoded_bytes:e.decodedBodySize,
            encoded_bytes:e.encodedBodySize,server_timing:e.serverTiming.map(t=>({name:t.name,ms:t.duration}))});
        }
      });
      p.resourceObserver.observe({type:'resource',buffered:false});
    })()""".replace("CONFIG", config)
    finish = """(() => {
      const p=window.__loadBenchmark;
      p.domObserver.disconnect();p.resourceObserver.disconnect();
      const t=document.querySelector('.message-virtual-transcript');
      window.__loadBenchmarkResult={first_visible_ms:p.first-p.start,frame_proxy_ms:p.frameProxy-p.start,
        route:location.pathname,last_message_id:p.config.lastId,expected_sequence:p.config.sequence,
        bottom_gap:p.bottomGap,input_absent:!document.querySelector('textarea'),
        persisted_member_hydrated:!!document.querySelector('[data-testid="persisted-conversation-owner"]'),
        scroll:{top:t.scrollTop,height:t.scrollHeight,client:t.clientHeight},resources:p.resources};
    })()"""
    return {"action":"run_scenario","runs":1,"warmup":0,"throttle_rate":1,"gc_per_run":False,
            "reset":{"kind":"navigate","url":url},"steps":[
                {"kind":"wait_selector","selector":selector,"timeout":"30s"},
                {"kind":"eval","expression":install},
                {"kind":"click","selector":selector},
                {"kind":"wait_eval","expression":"!!window.__loadBenchmark.frameProxy","timeout":"40s"},
                {"kind":"eval","expression":finish}]}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture",type=Path,required=True)
    parser.add_argument("--url",default="http://127.0.0.1:8043/")
    args=parser.parse_args()
    print(json.dumps(scenario(args.fixture,args.url),indent=2))


if __name__ == "__main__":
    main()
