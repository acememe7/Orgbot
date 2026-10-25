import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../workspace/desktop_files.dart';

class DesktopProjectPane extends StatefulWidget {
  const DesktopProjectPane({
    super.key,
    required this.projectPath,
    this.roomId,
    this.onProjectSelected,
  });

  final String? projectPath;
  final String? roomId;
  final ValueChanged<String>? onProjectSelected;

  @override
  State<DesktopProjectPane> createState() => _DesktopProjectPaneState();
}

class _DesktopProjectPaneState extends State<DesktopProjectPane> {
  static const _maxEntries = 500;
  static const _imageExtensions = {
    'png',
    'jpg',
    'jpeg',
    'gif',
    'webp',
    'bmp',
    'tif',
    'tiff',
    'avif',
    'ico',
  };
  static const _videoExtensions = {
    'mp4',
    'mkv',
    'webm',
    'mov',
    'avi',
    'm4v',
    'mpeg',
    'mpg',
    'ogv',
    '3gp',
    'flv',
  };
  static const _textExtensions = {
    'txt',
    'md',
    'markdown',
    'log',
    'json',
    'jsonl',
    'yaml',
    'yml',
    'toml',
    'rs',
    'dart',
    'py',
    'c',
    'cc',
    'cpp',
    'h',
    'hpp',
    'js',
    'ts',
    'html',
    'css',
    'xml',
    'csv',
    'ini',
    'conf',
    'sh',
    'sql',
    'java',
    'kt',
    'gradle',
    'cmake',
    'lock',
    'env',
  };

  String? _root;
  String? _current;
  List<_ProjectEntry> _entries = const [];
  String? _selected;
  String? _previewText;
  Uint8List? _previewBytes;
  String? _thumbnail;
  String? _error;
  String? _gitError;
  String _gitView = 'status';
  String _gitOutput = '';
  bool _loading = false;
  bool _truncated = false;
  bool _showGit = false;

  @override
  void initState() {
    super.initState();
    final path = widget.projectPath?.trim();
    if (path != null && path.isNotEmpty) {
      unawaited(_setRoot(path).then<void>((_) {}));
    }
  }

  @override
  void didUpdateWidget(covariant DesktopProjectPane oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.projectPath != widget.projectPath) {
      final path = widget.projectPath?.trim();
      if (path == null || path.isEmpty) {
        setState(() {
          _root = null;
          _current = null;
          _entries = const [];
          _selected = null;
          _previewBytes = null;
          _previewText = null;
        });
      } else {
        unawaited(_setRoot(path).then<void>((_) {}));
      }
    }
  }

  @override
  void dispose() {
    unawaited(_removeThumbnail());
    super.dispose();
  }

  Future<void> _chooseRoot() async {
    try {
      final selected = await DesktopFiles.chooseProjectFolder();
      if (selected != null && mounted) {
        final accepted = await _setRoot(selected);
        if (accepted && mounted) widget.onProjectSelected?.call(_root!);
      }
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  Future<bool> _setRoot(String path) async {
    try {
      final canonical = await Directory(path).resolveSymbolicLinks();
      if (!await Directory(canonical).exists()) {
        throw const FileSystemException('Choose an existing project folder.');
      }
      if (!mounted) return false;
      setState(() {
        _root = canonical;
        _current = canonical;
        _selected = null;
        _previewText = null;
        _previewBytes = null;
        _error = null;
      });
      await _refresh();
      return _error == null;
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
      return false;
    }
  }

  Future<void> _refresh() async {
    final root = _root;
    final current = _current;
    if (root == null || current == null) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final canonical = await Directory(current).resolveSymbolicLinks();
      _ensureWithinRoot(root, canonical);
      final entities = <_ProjectEntry>[];
      var truncated = false;
      await for (final entity in Directory(
        canonical,
      ).list(followLinks: false)) {
        if (entities.length == _maxEntries) {
          truncated = true;
          break;
        }
        final type = await FileSystemEntity.type(
          entity.path,
          followLinks: false,
        );
        final name = entity.path.split(Platform.pathSeparator).last;
        entities.add(
          _ProjectEntry(
            path: entity.path,
            name: name,
            directory: type == FileSystemEntityType.directory,
            symlink: type == FileSystemEntityType.link,
            regularFile: type == FileSystemEntityType.file,
          ),
        );
      }
      entities.sort((a, b) {
        final byType = (a.directory ? 0 : 1) - (b.directory ? 0 : 1);
        return byType != 0
            ? byType
            : a.name.toLowerCase().compareTo(b.name.toLowerCase());
      });
      if (!mounted) return;
      setState(() {
        _current = canonical;
        _entries = entities;
        _truncated = truncated;
      });
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _select(_ProjectEntry entry) async {
    if (entry.symlink || (!entry.directory && !entry.regularFile)) return;
    if (entry.directory) {
      final canonical = await Directory(entry.path).resolveSymbolicLinks();
      _ensureWithinRoot(_root!, canonical);
      setState(() {
        _current = canonical;
        _selected = null;
        _previewText = null;
        _previewBytes = null;
      });
      await _removeThumbnail();
      await _refresh();
      return;
    }
    try {
      final canonical = await File(entry.path).resolveSymbolicLinks();
      _ensureWithinRoot(_root!, canonical);
      setState(() {
        _selected = canonical;
        _previewText = null;
        _previewBytes = null;
        _error = null;
      });
      await _removeThumbnail();
      final ext = _extension(canonical);
      if (_imageExtensions.contains(ext) ||
          _videoExtensions.contains(ext) ||
          _textExtensions.contains(ext)) {
        final bytes = await DesktopFiles.readProjectFile(_root!, canonical);
        if (_imageExtensions.contains(ext)) {
          if (mounted) setState(() => _previewBytes = bytes);
        } else if (_videoExtensions.contains(ext)) {
          final thumbnail = await DesktopFiles.makeVideoThumbnailFromBytes(
            bytes,
          );
          if (mounted) {
            setState(() {
              _previewBytes = bytes;
              _thumbnail = thumbnail;
            });
          }
        } else {
          final preview = DesktopFiles.textPreview(bytes);
          if (mounted) {
            setState(() {
              _previewText = preview;
              if (preview == null) {
                _error = 'Text preview is limited to 512 KiB.';
              }
            });
          }
        }
      } else {
        if (mounted) {
          setState(
            () => _error = 'No internal preview is available for this file.',
          );
        }
      }
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  Future<void> _openSelected() async {
    final path = _selected;
    if (path == null) return;
    try {
      _ensureWithinRoot(_root!, await File(path).resolveSymbolicLinks());
      await DesktopFiles.openWith(path);
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  Future<void> _removeThumbnail() async {
    final path = _thumbnail;
    _thumbnail = null;
    if (path == null) return;
    final file = File(path);
    if (await file.exists()) await file.parent.delete(recursive: true);
  }

  Future<void> _loadGit(String view) async {
    final root = _root;
    if (root == null) return;
    setState(() {
      _gitView = view;
      _loading = true;
      _gitError = null;
      _gitOutput = '';
    });
    final args = switch (view) {
      'diff' => const [
        'diff',
        '--no-ext-diff',
        '--no-textconv',
        '--no-color',
        '--stat',
        'HEAD',
      ],
      'history' => const [
        'log',
        '-30',
        '--format=%h %s',
        '--no-show-signature',
      ],
      _ => const [
        '--no-optional-locks',
        'status',
        '--short',
        '--branch',
        '--untracked-files=no',
      ],
    };
    try {
      final output = await DesktopFiles.boundedGitOutput(root, args);
      if (mounted) {
        setState(
          () => _gitOutput = output.trim().isEmpty ? 'No changes.' : output,
        );
      }
    } catch (error) {
      if (mounted) setState(() => _gitError = '$error');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  void _ensureWithinRoot(String root, String path) {
    if (path != root && !path.startsWith('$root${Platform.pathSeparator}')) {
      throw const FileSystemException('This path leaves the selected project.');
    }
  }

  String _extension(String path) =>
      path.contains('.') ? path.split('.').last.toLowerCase() : '';

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final root = _root;
    if (root == null) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.folder_open_outlined, size: 42, color: scheme.primary),
            const SizedBox(height: 12),
            Text('Choose a project folder to browse files and Git history.'),
            const SizedBox(height: 12),
            FilledButton.icon(
              onPressed: _chooseRoot,
              icon: const Icon(Icons.folder_open),
              label: const Text('Choose folder'),
            ),
          ],
        ),
      );
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        final wide = constraints.maxWidth >= 760;
        final files = _fileBrowser(scheme);
        final preview = _preview(scheme);
        return Column(
          children: [
            _header(scheme),
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 0, 12, 10),
              child: Row(
                children: [
                  Expanded(
                    child: SegmentedButton<bool>(
                      segments: const [
                        ButtonSegment(
                          value: false,
                          label: Text('Files'),
                          icon: Icon(Icons.folder_outlined),
                        ),
                        ButtonSegment(
                          value: true,
                          label: Text('Git'),
                          icon: Icon(Icons.history),
                        ),
                      ],
                      selected: {_showGit},
                      onSelectionChanged: (selection) {
                        setState(() => _showGit = selection.first);
                        if (selection.first &&
                            _gitOutput.isEmpty &&
                            _gitError == null) {
                          unawaited(_loadGit('status'));
                        }
                      },
                    ),
                  ),
                ],
              ),
            ),
            Expanded(
              child: _showGit
                  ? _gitPanel(scheme)
                  : wide
                  ? Row(
                      children: [
                        Expanded(child: files),
                        Expanded(child: preview),
                      ],
                    )
                  : Column(
                      children: [
                        Expanded(child: files),
                        Expanded(child: preview),
                      ],
                    ),
            ),
          ],
        );
      },
    );
  }

  Widget _header(ColorScheme scheme) {
    final current = _current ?? _root ?? '';
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 8),
      child: Row(
        children: [
          Icon(Icons.folder_copy_outlined, size: 18, color: scheme.primary),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              current,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.labelLarge,
            ),
          ),
          IconButton(
            tooltip: 'Choose another project folder',
            onPressed: _chooseRoot,
            icon: const Icon(Icons.folder_open_outlined),
          ),
          IconButton(
            tooltip: 'Refresh',
            onPressed: _loading
                ? null
                : _showGit
                ? () => _loadGit(_gitView)
                : _refresh,
            icon: const Icon(Icons.refresh),
          ),
        ],
      ),
    );
  }

  Widget _fileBrowser(ColorScheme scheme) => Column(
    children: [
      if (_current != _root)
        ListTile(
          dense: true,
          leading: const Icon(Icons.arrow_upward),
          title: const Text('Parent folder'),
          onTap: _loading
              ? null
              : () async {
                  final parent = Directory(_current!).parent.path;
                  _ensureWithinRoot(_root!, parent);
                  setState(() => _current = parent);
                  await _refresh();
                },
        ),
      if (_loading) const LinearProgressIndicator(minHeight: 2),
      if (_error != null)
        Padding(
          padding: const EdgeInsets.all(12),
          child: Text(_error!, style: TextStyle(color: scheme.error)),
        ),
      Expanded(
        child: ListView.builder(
          itemCount: _entries.length + (_truncated ? 1 : 0),
          itemBuilder: (context, index) {
            if (index == _entries.length) {
              return const ListTile(
                title: Text('Showing the first 500 entries.'),
              );
            }
            final entry = _entries[index];
            return ListTile(
              dense: true,
              selected: entry.path == _selected,
              leading: Icon(
                entry.symlink
                    ? Icons.link_off
                    : entry.directory
                    ? Icons.folder_outlined
                    : Icons.insert_drive_file_outlined,
              ),
              title: Text(
                entry.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              subtitle: entry.symlink
                  ? const Text('Symlink not followed')
                  : null,
              enabled: !entry.symlink && (entry.directory || entry.regularFile),
              onTap: _loading ? null : () => _select(entry),
            );
          },
        ),
      ),
    ],
  );

  Widget _preview(ColorScheme scheme) {
    final path = _selected;
    final ext = path == null ? '' : _extension(path);
    Widget body;
    if (path == null) {
      body = Center(
        child: Text(
          'Select a file to preview or open it.',
          style: TextStyle(color: scheme.onSurfaceVariant),
        ),
      );
    } else if (_imageExtensions.contains(ext) && _previewBytes != null) {
      body = Center(
        child: InteractiveViewer(
          child: Image.memory(
            _previewBytes!,
            cacheWidth: 1024,
            cacheHeight: 1024,
            fit: BoxFit.contain,
            errorBuilder: (_, _, _) => _unsupportedPreview(scheme),
          ),
        ),
      );
    } else if (_videoExtensions.contains(ext)) {
      final thumbnail = _thumbnail;
      body = Center(
        child: thumbnail == null
            ? Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.movie_outlined, size: 48, color: scheme.primary),
                  const SizedBox(height: 8),
                  Text(
                    'Video preview unavailable. Open the original to play it.',
                  ),
                ],
              )
            : Image.file(File(thumbnail), fit: BoxFit.contain),
      );
    } else if (_previewText != null) {
      body = SingleChildScrollView(
        padding: const EdgeInsets.all(14),
        child: SelectableText(
          _previewText!,
          style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
        ),
      );
    } else {
      body = Center(child: _unsupportedPreview(scheme));
    }
    return Column(
      children: [
        Container(
          alignment: Alignment.centerLeft,
          padding: const EdgeInsets.fromLTRB(14, 8, 8, 8),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  path?.split(Platform.pathSeparator).last ?? 'Preview',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.titleSmall,
                ),
              ),
              if (path != null)
                TextButton.icon(
                  onPressed: _openSelected,
                  icon: const Icon(Icons.open_in_new, size: 18),
                  label: const Text('Open With'),
                ),
            ],
          ),
        ),
        if (_error != null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: Text(_error!, style: TextStyle(color: scheme.error)),
          ),
        Expanded(
          child: Container(
            margin: const EdgeInsets.fromLTRB(8, 0, 8, 8),
            decoration: BoxDecoration(
              color: scheme.surfaceContainerLow,
              border: Border.all(color: scheme.outlineVariant),
              borderRadius: BorderRadius.circular(10),
            ),
            child: body,
          ),
        ),
      ],
    );
  }

  Widget _unsupportedPreview(ColorScheme scheme) => Column(
    mainAxisSize: MainAxisSize.min,
    children: [
      Icon(Icons.insert_drive_file_outlined, size: 42, color: scheme.primary),
      const SizedBox(height: 8),
      const Text('No internal preview for this file.'),
      const SizedBox(height: 4),
      const Text('Use Open With to launch a desktop application.'),
    ],
  );

  Widget _gitPanel(ColorScheme scheme) => Column(
    children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
        child: SegmentedButton<String>(
          segments: const [
            ButtonSegment(value: 'status', label: Text('Status')),
            ButtonSegment(value: 'diff', label: Text('Diff')),
            ButtonSegment(value: 'history', label: Text('History')),
          ],
          selected: {_gitView},
          onSelectionChanged: (selection) => _loadGit(selection.first),
        ),
      ),
      if (_loading) const LinearProgressIndicator(minHeight: 2),
      if (_gitError != null)
        Padding(
          padding: const EdgeInsets.all(12),
          child: Text(_gitError!, style: TextStyle(color: scheme.error)),
        ),
      Expanded(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(14),
          child: SelectableText(
            _gitOutput.isEmpty && _gitError == null
                ? 'Loading Git…'
                : _gitOutput,
            style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
          ),
        ),
      ),
    ],
  );
}

class _ProjectEntry {
  const _ProjectEntry({
    required this.path,
    required this.name,
    required this.directory,
    required this.symlink,
    required this.regularFile,
  });

  final String path;
  final String name;
  final bool directory;
  final bool symlink;
  final bool regularFile;
}
