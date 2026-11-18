#!/usr/bin/env bash
# Serves the production build over HTTPS on the local network so a phone on the
# same Wi-Fi can open the studio (camera needs HTTPS) and the player.
#   http://<IP>:5480   — start page: CA certificate, trust check, links (open this first)
#   https://<IP>:5443  — the site (API/media proxied to the dev server on :8080)
# Requires ./scripts/dev-server.sh running. Keys stay in .certs/ (git-ignored).
set -euo pipefail
cd "$(dirname "$0")/.."
IP="${PHONE_TEST_IP:-$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1)}"
DIR="$PWD/.certs"
mkdir -p "$DIR/public"
chmod 700 "$DIR"

if [[ ! -f "$DIR/ca.key" ]]; then
  # The CA may only vouch for private-network addresses: useless for anything else
  # even if this key leaked.
  openssl req -x509 -new -nodes -newkey rsa:2048 -sha256 -days 825 \
    -keyout "$DIR/ca.key" -out "$DIR/ca.crt" -subj "/CN=SchoolLive Dev CA" \
    -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -addext "nameConstraints=critical,permitted;IP:192.168.0.0/255.255.0.0,permitted;IP:10.0.0.0/255.0.0.0,permitted;IP:172.16.0.0/255.240.0.0,permitted;DNS:localhost" \
    2>/dev/null
fi
if [[ ! -f "$DIR/$IP.crt" ]]; then
  openssl req -new -nodes -newkey rsa:2048 -keyout "$DIR/$IP.key" -out "$DIR/$IP.csr" -subj "/CN=$IP" 2>/dev/null
  openssl x509 -req -in "$DIR/$IP.csr" -CA "$DIR/ca.crt" -CAkey "$DIR/ca.key" -CAcreateserial \
    -out "$DIR/$IP.crt" -days 397 -sha256 -extfile <(printf '%s\n' \
      "subjectAltName=IP:$IP,DNS:localhost" "extendedKeyUsage=serverAuth" \
      "keyUsage=critical,digitalSignature,keyEncipherment" "basicConstraints=CA:FALSE") 2>/dev/null
fi
cp "$DIR/ca.crt" "$DIR/public/schoollive-ca.crt"
FINGERPRINT="$(openssl x509 -in "$DIR/ca.crt" -noout -fingerprint -sha256 | cut -d= -f2)"

# Start page for the phone: certificate download, a live trust check, links.
cat > "$DIR/public/index.html" <<HTML
<!doctype html>
<html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>SchoolLive: проверка на телефоне</title>
<style>
:root{color-scheme:light dark;--bg:#f4f6fb;--card:#fff;--ink:#0f1830;--muted:#66718a;--blue:#3651a3;--ok:#23824a;--bad:#c42139}
@media (prefers-color-scheme:dark){:root{--bg:#0b1120;--card:#121a2d;--ink:#eaf0fc;--muted:#93a0ba;--blue:#7b93ff;--ok:#4cc27b;--bad:#ff8090}}
body{margin:0;background:var(--bg);color:var(--ink);font:16px/1.5 system-ui,-apple-system,sans-serif}
main{max-width:560px;margin:0 auto;padding:20px 16px 40px}
h1{font-size:24px;margin:8px 0 16px}.card{background:var(--card);border-radius:18px;padding:18px;margin:14px 0}
h2{font-size:17px;margin:0 0 8px}ol{margin:8px 0 0;padding-left:20px}li{margin:4px 0}small,.muted{color:var(--muted)}
a.btn,button{display:block;width:100%;box-sizing:border-box;text-align:center;padding:14px;border:0;border-radius:14px;background:var(--blue);color:#fff;font:600 17px system-ui;text-decoration:none;margin-top:12px}
.status{font-weight:600}.ok{color:var(--ok)}.bad{color:var(--bad)}code{word-break:break-all;font-size:12px}
</style></head><body><main>
<h1>Проверка SchoolLive на телефоне</h1>
<div class="card"><h2>1. Сертификат</h2>
<a class="btn" href="/schoollive-ca.crt">Скачать сертификат</a>
<p><b>iPhone:</b></p><ol>
<li>После скачивания: <b>Настройки → Основные → VPN и управление устройством</b> → «SchoolLive Dev CA» → Установить.</li>
<li><b>Настройки → Основные → Об этом устройстве → Доверие сертификатам</b> (в самом низу) → включить «SchoolLive Dev CA».</li></ol>
<p><b>Android:</b> Настройки → Безопасность → Шифрование и учётные данные → Установить сертификат → Сертификат ЦС.</p>
<small>Отпечаток SHA-256: <code>$FINGERPRINT</code></small></div>
<div class="card"><h2>2. Проверка</h2>
<p id="status" class="status muted">Проверяем…</p>
<button id="retry" type="button">Проверить снова</button></div>
<div class="card"><h2>3. Открыть</h2>
<a class="btn" href="https://$IP:5443/admin">Студия (вход администрации)</a>
<a class="btn" href="https://$IP:5443/">Сайт для зрителей</a>
<p class="muted">Код администрации для разработки: <b>school1448</b></p></div>
</main><script>
const st=document.getElementById('status');
async function check(){st.className='status muted';st.textContent='Проверяем…';
try{await fetch('https://$IP:5443/api/health?'+Date.now(),{mode:'no-cors',cache:'no-store'});
st.className='status ok';st.textContent='✓ Телефон доверяет сертификату, сайт доступен по HTTPS.'}
catch(e){st.className='status bad';st.textContent='✗ HTTPS не открывается: сертификат ещё не установлен или не включено «Доверие сертификатам» (шаг 1). Если всё сделано — сеть блокирует порт 5443 (отключите VPN на Mac).'}}
document.getElementById('retry').onclick=check;check();
</script></body></html>
HTML

npm --prefix web run build >/dev/null

# Plain HTTP only for downloading the public CA certificate.
python3 -m http.server 5480 --bind 0.0.0.0 --directory "$DIR/public" >/dev/null 2>&1 &
CERT_SERVER=$!
trap 'kill $CERT_SERVER 2>/dev/null' EXIT

# Port 5443 answers both: TLS goes to Vite, plain http:// is redirected to https://.
node scripts/https-mux.mjs 5443 5444 &
MUX=$!
trap 'kill $CERT_SERVER $MUX 2>/dev/null' EXIT

echo "Откройте на телефоне:  http://$IP:5480"
echo "Сайт:                  https://$IP:5443"
cd web
HTTPS_KEY="$DIR/$IP.key" HTTPS_CERT="$DIR/$IP.crt" npx vite preview --host 127.0.0.1 --port 5444 --strictPort
