import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'workspace.dart';

Future<bool> trustWorkspaceHost(
  BuildContext context,
  String host,
  String type,
  String fingerprint,
) async =>
    await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (context) => AlertDialog(
        title: const Text('Verify server identity'),
        content: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Compare this fingerprint with your server administrator or VPS console before connecting to $host.',
              ),
              const SizedBox(height: 16),
              Text(type),
              SelectableText(fingerprint),
              const SizedBox(height: 12),
              const Text(
                'The saved identity will be required on every future connection.',
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Trust this server'),
          ),
        ],
      ),
    ) ??
    false;

class WorkspaceScreen extends StatefulWidget {
  final WorkspaceConfig config;
  final String roomId;
  final Future<void> Function(WorkspaceConfig)? onSave;
  const WorkspaceScreen({
    super.key,
    required this.config,
    required this.roomId,
    this.onSave,
  });
  @override
  State<WorkspaceScreen> createState() => _WorkspaceScreenState();
}

class _WorkspaceScreenState extends State<WorkspaceScreen> {
  late final WorkspaceConfig _config = widget.config.copy();
  late final _host = TextEditingController(text: _config.host);
  late final _port = TextEditingController(text: _config.port.toString());
  late final _user = TextEditingController(text: _config.username);
  late final _password = TextEditingController(text: _config.password);
  late final _key = TextEditingController(text: _config.privateKey);
  late final _passphrase = TextEditingController(text: _config.passphrase);
  late final _path = TextEditingController(text: _config.path);
  late bool _keyAuth = _config.privateKey.isNotEmpty;
  bool _busy = false;
  String? _error;
  WorkspaceConnection? _connection;
  @override
  void dispose() {
    _connection?.close();
    for (final controller in [
      _host,
      _port,
      _user,
      _password,
      _key,
      _passphrase,
      _path,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  void _read() {
    final host = _host.text.trim();
    final port = int.tryParse(_port.text) ?? 0;
    if (host != _config.host || port != _config.port) _config.fingerprint = '';
    _config.host = host;
    _config.port = port;
    _config.username = _user.text.trim();
    _config.password = _keyAuth ? '' : _password.text;
    _config.privateKey = _keyAuth ? _key.text : '';
    _config.passphrase = _keyAuth ? _passphrase.text : '';
    _config.path = _path.text.trim();
  }

  Future<void> _loadKey({required bool clipboard}) async {
    try {
      final text = clipboard
          ? (await Clipboard.getData(Clipboard.kTextPlain))?.text
          : (await workspaceChannel.invokeMapMethod<String, Object?>(
                  'import',
                ))?['content']
                as String?;
      if (text == null || !mounted) return;
      if (text.length > 65536 ||
          !RegExp(
            r'^-----BEGIN (OPENSSH|RSA|EC) PRIVATE KEY-----\r?\n',
          ).hasMatch(text.trim())) {
        throw const FormatException(
          'Choose a complete OpenSSH/PEM private key, including its BEGIN and END lines. A .pub file is a public key.',
        );
      }
      setState(() {
        _key.text = text.trim();
        _error = null;
      });
    } catch (_) {
      if (mounted) {
        setState(
          () => _error =
              'Could not read a private key. Use a complete OpenSSH/PEM private-key file or clipboard text, including its BEGIN and END lines.',
        );
      }
    }
  }

  Future<void> _save() async {
    _read();
    setState(() {
      _busy = true;
      _error = null;
    });
    final connection = WorkspaceConnection(_config, widget.roomId);
    _connection = connection;
    try {
      await connection.connect(
        trust: (host, type, key) async =>
            mounted && await trustWorkspaceHost(context, host, type, key),
      );
      // The remote root is checked by the SFTP server, including access rights.
      _config.path = connection.root;
      await widget.onSave?.call(_config);
      if (mounted) Navigator.pop(context, _config);
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      await connection.close();
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _browse() async {
    _read();
    final browsing = _config.copy();
    // This is a read-only folder picker. Bots receive only the selected root.
    browsing.writable = false;
    browsing.commands = false;
    browsing.path = browsing.isRemote
        ? '/'
        : await WorkspaceConnection.phoneBase();
    if (!browsing.isRemote) {
      await Directory(browsing.path).create(recursive: true);
    }
    if (!mounted) return;
    final selected = await Navigator.push<WorkspaceConfig>(
      context,
      MaterialPageRoute(
        builder: (_) => WorkspaceFilesScreen(
          config: browsing,
          roomId: widget.roomId,
          selectFolder: true,
        ),
      ),
    );
    if (selected != null && mounted) {
      setState(() {
        _path.text = selected.path;
        _config.fingerprint = selected.fingerprint;
      });
    }
  }

  Widget _field(
    TextEditingController c,
    String label, {
    bool secret = false,
    bool readOnly = false,
    int lines = 1,
    TextInputType? keyboard,
  }) => Padding(
    padding: const EdgeInsets.only(bottom: 14),
    child: TextField(
      controller: c,
      enabled: !_busy,
      obscureText: secret,
      readOnly: readOnly,
      enableSuggestions: !secret,
      autocorrect: false,
      maxLines: lines,
      keyboardType: keyboard,
      decoration: InputDecoration(
        labelText: label,
        border: const OutlineInputBorder(),
      ),
    ),
  );
  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('Working computer')),
    body: AbsorbPointer(
      absorbing: _busy,
      child: ListView(
        padding: const EdgeInsets.all(20),
        children: [
          SegmentedButton<String>(
            segments: const [
              ButtonSegment(
                value: 'phone',
                icon: Icon(Icons.phone_android),
                label: Text('This phone'),
              ),
              ButtonSegment(
                value: 'ssh',
                icon: Icon(Icons.dns_outlined),
                label: Text('SSH / VPS'),
              ),
            ],
            selected: {_config.kind},
            onSelectionChanged: (value) => setState(() {
              _config.kind = value.first;
              _path.clear();
            }),
          ),
          const SizedBox(height: 20),
          Text(
            _config.isRemote
                ? 'Models run in this Android app. Files and commands run on the SSH computer. No OrgBot server or model CLI is required there.'
                : 'Bots work with files on this phone. Each room gets a persistent folder by default. Phone workspaces provide file tools; shell execution is available with SSH.',
          ),
          const SizedBox(height: 20),
          if (_config.isRemote) ...[
            _field(_host, 'Hostname or IP address'),
            _field(_port, 'SSH port', keyboard: TextInputType.number),
            _field(_user, 'Username'),
            SegmentedButton<bool>(
              segments: const [
                ButtonSegment(value: false, label: Text('Password')),
                ButtonSegment(value: true, label: Text('Private key')),
              ],
              selected: {_keyAuth},
              onSelectionChanged: (value) =>
                  setState(() => _keyAuth = value.first),
            ),
            const SizedBox(height: 16),
            if (_keyAuth) ...[
              // A single-line editable field strips PEM newlines on paste.
              // Set the controller directly from an explicit import/paste action.
              _field(
                _key,
                'OpenSSH / PEM private key',
                secret: true,
                readOnly: true,
              ),
              Wrap(
                spacing: 12,
                children: [
                  OutlinedButton.icon(
                    onPressed: () => _loadKey(clipboard: false),
                    icon: const Icon(Icons.file_open_outlined),
                    label: const Text('Import key file'),
                  ),
                  OutlinedButton.icon(
                    onPressed: () => _loadKey(clipboard: true),
                    icon: const Icon(Icons.content_paste),
                    label: const Text('Paste key'),
                  ),
                ],
              ),
              const SizedBox(height: 16),
              _field(
                _passphrase,
                'Key passphrase (if encrypted)',
                secret: true,
              ),
            ] else
              _field(_password, 'SSH password', secret: true),
          ],
          _field(
            _path,
            _config.isRemote
                ? 'Remote folder (absolute path)'
                : 'Phone folder (blank for room storage)',
          ),
          Align(
            alignment: Alignment.centerLeft,
            child: OutlinedButton.icon(
              onPressed: _busy ? null : _browse,
              icon: const Icon(Icons.folder_open),
              label: const Text('Browse folders'),
            ),
          ),
          if (!_config.isRemote)
            const Text(
              'Custom folders must already exist and be accessible to Android apps. Use Files to import or export documents from shared storage.',
            ),
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            title: const Text('Allow file changes'),
            subtitle: const Text('Disable for a read-only workspace.'),
            value: _config.writable,
            onChanged: (v) => setState(() => _config.writable = v),
          ),
          if (_config.isRemote)
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Allow shell commands'),
              subtitle: const Text(
                'Commands have this SSH account’s permissions beyond the working folder. Use an account with the access you intend to give bots.',
              ),
              value: _config.commands && _config.writable,
              onChanged: _config.writable
                  ? (v) => setState(() => _config.commands = v)
                  : null,
            ),
          if (_config.isRemote && _config.fingerprint.isNotEmpty) ...[
            const SizedBox(height: 12),
            const Text('Trusted server fingerprint'),
            SelectableText(_config.fingerprint),
            TextButton(
              onPressed: () async {
                final reset = await showDialog<bool>(
                  context: context,
                  builder: (context) => AlertDialog(
                    title: const Text('Replace trusted identity?'),
                    content: const Text(
                      'Only do this after independently verifying that your server key changed. The next connection will ask you to verify the new fingerprint.',
                    ),
                    actions: [
                      TextButton(
                        onPressed: () => Navigator.pop(context, false),
                        child: const Text('Cancel'),
                      ),
                      TextButton(
                        onPressed: () => Navigator.pop(context, true),
                        child: const Text('Verify a new key'),
                      ),
                    ],
                  ),
                );
                if (reset == true && mounted) {
                  setState(() => _config.fingerprint = '');
                }
              },
              child: const Text('Verify a changed server key'),
            ),
          ],
          if (_error != null)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 12),
              child: Text(
                _error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            ),
          const SizedBox(height: 16),
          FilledButton.icon(
            onPressed: _save,
            icon: _busy
                ? const SizedBox(
                    width: 18,
                    height: 18,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                : const Icon(Icons.check),
            label: Text(_busy ? 'Connecting…' : 'Test connection and save'),
          ),
          const SizedBox(height: 16),
          const Text(
            'Connection credentials are saved in encrypted device storage.',
          ),
        ],
      ),
    ),
  );
}

class WorkspaceFilesScreen extends StatefulWidget {
  final WorkspaceConfig config;
  final String roomId;
  final bool selectFolder, embedded;
  const WorkspaceFilesScreen({
    super.key,
    required this.config,
    required this.roomId,
    this.selectFolder = false,
    this.embedded = false,
  });
  @override
  State<WorkspaceFilesScreen> createState() => _WorkspaceFilesScreenState();
}

class _WorkspaceFilesScreenState extends State<WorkspaceFilesScreen> {
  WorkspaceConnection? _connection;
  List<Map<String, Object?>> _entries = [];
  String _path = '.';
  String? _error;
  bool _busy = true;
  @override
  void initState() {
    super.initState();
    _connect();
  }

  @override
  void dispose() {
    _connection?.close();
    super.dispose();
  }

  Future<void> _connect() async {
    await _connection?.close();
    if (!mounted) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    final connection = WorkspaceConnection(widget.config.copy(), widget.roomId);
    _connection = connection;
    try {
      // Settings must establish trust first; this viewer cannot replace a key.
      await connection.connect(
        trust: widget.selectFolder
            ? (host, type, key) async =>
                  mounted && await trustWorkspaceHost(context, host, type, key)
            : null,
      );
      if (!mounted) {
        await connection.close();
        return;
      }
      await _load();
    } catch (e) {
      if (mounted) {
        setState(() {
          _error = e.toString();
          _busy = false;
        });
      }
    }
  }

  Future<void> _load() async {
    if (!mounted) return;
    setState(() {
      _busy = true;
      _error = null;
      _entries = [];
    });
    try {
      final list =
          await _connection!.execute('workspace_list', {'path': _path}) as List;
      if (mounted) {
        setState(() {
          _entries = list.cast<Map<String, Object?>>()
            ..sort((a, b) {
              final directories =
                  (a['directory'] == true ? 0 : 1) -
                  (b['directory'] == true ? 0 : 1);
              return directories != 0
                  ? directories
                  : (a['name'] as String).toLowerCase().compareTo(
                      (b['name'] as String).toLowerCase(),
                    );
            });
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  String _child(String name) => _path == '.' ? name : '$_path/$name';
  Future<void> _edit({String? name, String? initial}) async {
    try {
      final body =
          initial ??
          (name == null
              ? ''
              : await _connection!.execute('workspace_read', {
                      'path': _child(name),
                    })
                    as String);
      if (!mounted) return;
      final result = await Navigator.push<(String, String)>(
        context,
        MaterialPageRoute(
          builder: (_) => _FileEditor(
            name: name ?? '',
            content: body,
            readOnly: !widget.config.writable,
            existing: name != null && initial == null,
          ),
        ),
      );
      if (result != null) {
        await _connection!.execute('workspace_write', {
          'path': _child(result.$1),
          'content': result.$2,
        });
        await _load();
      }
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  Future<void> _import() async {
    try {
      final file = await workspaceChannel.invokeMapMethod<String, Object?>(
        'import',
      );
      if (file != null) {
        await _edit(
          name: file['name'] as String,
          initial: file['content'] as String,
        );
      }
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(
      automaticallyImplyLeading: !widget.embedded,
      toolbarHeight: widget.embedded ? 48 : kToolbarHeight,
      title: Text(
        widget.selectFolder ? 'Choose a working folder' : 'Workspace files',
        style: TextStyle(fontSize: widget.embedded ? 15 : 18),
      ),
      actions: [
        IconButton(
          tooltip: 'Reconnect',
          onPressed: _busy ? null : _connect,
          icon: const Icon(Icons.refresh),
        ),
        if (widget.config.writable) ...[
          if (Platform.isAndroid)
            IconButton(
              tooltip: 'Import text file',
              onPressed: _busy ? null : _import,
              icon: const Icon(Icons.file_download_outlined),
            ),
          IconButton(
            tooltip: 'New text file',
            onPressed: _busy ? null : () => _edit(),
            icon: const Icon(Icons.note_add_outlined),
          ),
        ],
      ],
    ),
    body: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (widget.selectFolder)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 0),
            child: FilledButton.icon(
              onPressed: _busy || _error != null
                  ? null
                  : () {
                      final selected = _connection!.config.copy();
                      selected.path = _path == '.'
                          ? _connection!.root
                          : '${_connection!.root == '/' ? '' : _connection!.root}/$_path';
                      Navigator.pop(context, selected);
                    },
              icon: const Icon(Icons.check),
              label: const Text('Use this folder'),
            ),
          ),
        Padding(
          padding: const EdgeInsets.all(16),
          child: SelectableText(
            _path == '.'
                ? (_connection?.root ?? widget.config.path)
                : '${_connection?.root ?? widget.config.path}/$_path',
          ),
        ),
        if (_error != null)
          Padding(
            padding: const EdgeInsets.all(16),
            child: Text(
              _error!,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          ),
        if (_path != '.')
          ListTile(
            leading: const Icon(Icons.arrow_upward),
            title: const Text('Parent folder'),
            onTap: _busy
                ? null
                : () {
                    final parts = _path.split('/')..removeLast();
                    _path = parts.isEmpty ? '.' : parts.join('/');
                    _load();
                  },
          ),
        if (_busy) const LinearProgressIndicator(),
        Expanded(
          child: ListView(
            children: [
              if (!_busy && _entries.isEmpty && _error == null)
                const ListTile(title: Text('This folder is empty.')),
              for (final entry in _entries)
                ListTile(
                  leading: Icon(
                    entry['directory'] == true
                        ? Icons.folder_outlined
                        : Icons.description_outlined,
                  ),
                  title: Text(entry['name'] as String),
                  onTap: _busy
                      ? null
                      : () {
                          if (entry['directory'] == true) {
                            _path = _child(entry['name'] as String);
                            _load();
                          } else {
                            if (!widget.selectFolder) {
                              _edit(name: entry['name'] as String);
                            }
                          }
                        },
                ),
            ],
          ),
        ),
      ],
    ),
  );
}

class _FileEditor extends StatefulWidget {
  final String name, content;
  final bool readOnly, existing;
  const _FileEditor({
    required this.name,
    required this.content,
    required this.readOnly,
    required this.existing,
  });
  @override
  State<_FileEditor> createState() => _FileEditorState();
}

class _FileEditorState extends State<_FileEditor> {
  late final _name = TextEditingController(text: widget.name);
  late final _body = TextEditingController(text: widget.content);
  @override
  void dispose() {
    _name.dispose();
    _body.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(
      title: Text(widget.existing ? widget.name : 'New text file'),
      actions: [
        if (Platform.isAndroid)
          IconButton(
            tooltip: 'Export text file',
            icon: const Icon(Icons.ios_share),
            onPressed: () async {
              try {
                await workspaceChannel.invokeMethod('export', {
                  'name': _name.text,
                  'content': _body.text,
                });
              } catch (e) {
                if (context.mounted) {
                  ScaffoldMessenger.of(
                    context,
                  ).showSnackBar(SnackBar(content: Text('$e')));
                }
              }
            },
          ),
        if (!widget.readOnly)
          TextButton(
            onPressed: () {
              try {
                if (cleanPath(_name.text) == '.') return;
                Navigator.pop(context, (_name.text, _body.text));
              } catch (e) {
                ScaffoldMessenger.of(
                  context,
                ).showSnackBar(SnackBar(content: Text('$e')));
              }
            },
            child: const Text('Save'),
          ),
      ],
    ),
    body: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        children: [
          if (!widget.existing)
            TextField(
              controller: _name,
              decoration: const InputDecoration(labelText: 'File name'),
            ),
          const SizedBox(height: 12),
          Expanded(
            child: TextField(
              controller: _body,
              readOnly: widget.readOnly,
              expands: true,
              maxLines: null,
              minLines: null,
              autocorrect: false,
              keyboardType: TextInputType.multiline,
              style: const TextStyle(fontFamily: 'monospace', fontSize: 13),
              decoration: const InputDecoration(border: OutlineInputBorder()),
            ),
          ),
        ],
      ),
    ),
  );
}
