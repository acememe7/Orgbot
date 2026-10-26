import asyncio, asyncssh, pathlib, json, signal, secrets, base64
# Isolated local QA only. Never expose this test shell outside loopback.
import tempfile
root=pathlib.Path(tempfile.mkdtemp(prefix='orgbot-ssh-qa-'))
root.mkdir(exist_ok=True)
work=root/'work';work.mkdir(exist_ok=True)
(root/'outside.txt').write_text('outside workspace')
(work/'escape').symlink_to(root, target_is_directory=True) if not (work/'escape').exists() else None
(work/'existing.txt').write_text('original')
(work/'disconnect-once.txt').write_text('read recovered')
(work/'oversized').mkdir()
for i in range(501): (work/'oversized'/str(i)).touch()
fixture_password=secrets.token_urlsafe(32)
key=asyncssh.generate_private_key('ssh-ed25519')
client_key=asyncssh.generate_private_key('ssh-ed25519')
key_passphrase=secrets.token_urlsafe(24)
(root/'client_key').write_bytes(client_key.export_private_key('openssh',passphrase=key_passphrase))
class Server(asyncssh.SSHServer):
    def password_auth_supported(self): return self.username != 'qa-key'
    def connection_made(self, conn): self.username=''
    def begin_auth(self, username): self.username=username; return True
    def validate_password(self, username, password): return username=='qa' and secrets.compare_digest(password, fixture_password)
    def public_key_auth_supported(self): return True
    def validate_public_key(self, username, key): return username=='qa-key' and key==client_key.convert_to_public()
class SftpServer(asyncssh.SFTPServer):
    dropped_read = False
    async def open(self, path, pflags, attrs):
        if path.endswith(b'/disconnect-once.txt') and not SftpServer.dropped_read:
            SftpServer.dropped_read = True
            self.connection.abort()
            await asyncio.sleep(0.05)
        return super().open(path, pflags, attrs)

async def process(session):
    if 'orgbot-qa-disconnect' in session.command:
        with (work/'disconnect-marker').open('a') as output: output.write('once')
        session.channel.get_connection().abort()
        return
    child=await asyncio.create_subprocess_shell(session.command,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.PIPE,start_new_session=True)
    async def pump(stream,target):
        while data:=await stream.read(4096): target.write(data.decode())
    try:
        await asyncio.gather(pump(child.stdout,session.stdout),pump(child.stderr,session.stderr))
        session.exit(await child.wait())
    except (asyncio.CancelledError,asyncssh.SignalReceived):
        import os
        try: os.killpg(child.pid,signal.SIGKILL)
        except ProcessLookupError: pass
        raise
async def main():
    server=await asyncssh.create_server(Server,'127.0.0.1',22222,server_host_keys=[key],sftp_factory=SftpServer,process_factory=process)
    (root/'connection.json').write_text(json.dumps({'host':'127.0.0.1','port':22222,'path':str(work),'fingerprint':key.get_fingerprint(),'password':fixture_password,'key_path':str(root/'client_key'),'key_passphrase':key_passphrase}))
    (root/'device-defines.json').write_text(json.dumps({'SSH_TEST_PATH':str(work),'SSH_TEST_FINGERPRINT':key.get_fingerprint(),'SSH_TEST_PASSWORD':fixture_password,'SSH_TEST_PRIVATE_KEY_B64':base64.b64encode((root/'client_key').read_bytes()).decode(),'SSH_TEST_KEY_PASSPHRASE':key_passphrase}))
    print('SSH QA server ready; connection settings: '+str(root/'connection.json')+'; device defines: '+str(root/'device-defines.json'),flush=True)
    await server.wait_closed()
asyncio.run(main())
