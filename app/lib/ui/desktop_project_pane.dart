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
    @visibleForTesting this.initialRootForTesting,
    @visibleForTesting this.gitOutputForTesting,
  });

  final String? projectPath;
  final String? roomId;
  final ValueChanged<String>? onProjectSelected;

  @visibleForTesting
  final String? initialRootForTesting;

  @visibleForTesting
  final Future<String> Function(String root, List<String> args)?
  gitOutputForTesting;

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
  String? _browseError;
  String? _previewError;
  String? _gitError;
  String _gitView = 'status';
  String _gitOutput = '';
  bool _loading = false;
  bool _gitLoading = false;
  bool _previewOpen = false;
  bool _previewLoading = false;
  bool _truncated = false;
  bool _showGit = false;
  int _listGeneration = 0;
  int _previewGeneration = 0;
  int _gitGeneration = 0;

  @override
  void initState() {
    super.initState();
    final testRoot = widget.initialRootForTesting;
    if (testRoot != null) {
      _root = testRoot;
      _current = testRoot;
    }
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
        _listGeneration++;
        _previewGeneration++;
        _invalidateGit();
        setState(() {
          _root = null;
          _current = null;
          _entries = const [];
          _selected = null;
          _previewBytes = null;
          _previewText = null;
          _previewError = null;
          _browseError = null;
          _previewOpen = false;
        });
        unawaited(_removeThumbnail());
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
      if (mounted) setState(() => _browseError = '$error');
    }
  }

  Future<bool> _setRoot(String path) async {
    final generation = ++_listGeneration;
    _previewGeneration++;
    final hadRoot = _root != null;
    _invalidateGit();
    if (mounted && hadRoot) setState(() {});
    try {
      final canonical = await Directory(path).resolveSymbolicLinks();
      if (!await Directory(canonical).exists()) {
        throw const FileSystemException('Choose an existing project folder.');
      }
      if (!mounted || generation != _listGeneration) return false;
      setState(() {
        _root = canonical;
        _current = canonical;
        _selected = null;
        _previewText = null;
        _previewBytes = null;
        _previewError = null;
        _previewOpen = false;
        _browseError = null;
      });
      await _refresh();
      return _browseError == null;
    } catch (error) {
      if (mounted && generation == _listGeneration) {
        setState(() => _browseError = '$error');
      }
      return false;
    }
  }

  Future<void> _refresh() async {
    final root = _root;
    final current = _current;
    if (root == null || current == null) return;
    final generation = ++_listGeneration;
    setState(() {
      _loading = true;
      _browseError = null;
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
      if (!mounted || generation != _listGeneration) return;
      setState(() {
        _current = canonical;
        _entries = entities;
        _truncated = truncated;
      });
    } catch (error) {
      if (mounted && generation == _listGeneration) {
        setState(() => _browseError = '$error');
      }
    } finally {
      if (mounted && generation == _listGeneration) {
        setState(() => _loading = false);
      }
    }
  }

  Future<void> _select(_ProjectEntry entry) async {
    if (entry.symlink || (!entry.directory && !entry.regularFile)) return;
    final selection = ++_previewGeneration;
    if (entry.directory) {
      try {
        final canonical = await Directory(entry.path).resolveSymbolicLinks();
        _ensureWithinRoot(_root!, canonical);
        if (!mounted || selection != _previewGeneration) return;
        setState(() {
          _current = canonical;
          _selected = null;
          _previewText = null;
          _previewBytes = null;
          _previewError = null;
          _previewOpen = false;
          _previewLoading = false;
        });
        await _removeThumbnail();
        await _refresh();
      } catch (error) {
        if (mounted && selection == _previewGeneration) {
          setState(() => _browseError = '$error');
        }
      }
      return;
    }
    try {
      final canonical = await File(entry.path).resolveSymbolicLinks();
      _ensureWithinRoot(_root!, canonical);
      if (!mounted || selection != _previewGeneration) return;
      setState(() {
        _selected = canonical;
        _previewOpen = true;
      });
      await _loadPreview(canonical, selection);
    } catch (error) {
      if (mounted && selection == _previewGeneration) {
        setState(() => _previewError = '$error');
      }
    }
  }

  Future<void> _loadPreview(String path, int generation) async {
    final root = _root;
    if (root == null) return;
    setState(() {
      _previewText = null;
      _previewBytes = null;
      _previewError = null;
      _previewLoading = true;
    });
    await _removeThumbnail();
    if (!mounted || generation != _previewGeneration) return;
    try {
      final ext = _extension(path);
      if (!_imageExtensions.contains(ext) &&
          !_videoExtensions.contains(ext) &&
          !_textExtensions.contains(ext)) {
        setState(() {
          _previewError = 'No internal preview is available for this file.';
          _previewLoading = false;
        });
        return;
      }
      final bytes = await DesktopFiles.readProjectFile(root, path);
      if (!mounted || generation != _previewGeneration) return;
      if (_imageExtensions.contains(ext)) {
        setState(() {
          _previewBytes = bytes;
          _previewLoading = false;
        });
      } else if (_videoExtensions.contains(ext)) {
        final thumbnail = await DesktopFiles.makeVideoThumbnailFromBytes(bytes);
        if (!mounted || generation != _previewGeneration) {
          if (thumbnail != null) {
            final output = File(thumbnail);
            if (await output.exists()) {
              await output.parent.delete(recursive: true);
            }
          }
          return;
        }
        setState(() {
          _previewBytes = bytes;
          _thumbnail = thumbnail;
          _previewLoading = false;
        });
      } else {
        final preview = DesktopFiles.textPreview(bytes);
        setState(() {
          _previewText = preview;
          _previewError = preview == null
              ? 'Text preview is limited to 512 KiB.'
              : null;
          _previewLoading = false;
        });
      }
    } catch (error) {
      if (mounted && generation == _previewGeneration) {
        setState(() {
          _previewError = '$error';
          _previewLoading = false;
        });
      }
    }
  }

  void _togglePreview() {
    if (_selected == null) return;
    setState(() => _previewOpen = !_previewOpen);
  }

  Future<void> _openSelected() async {
    final path = _selected;
    if (path == null) return;
    try {
      _ensureWithinRoot(_root!, await File(path).resolveSymbolicLinks());
      await DesktopFiles.openWith(path);
    } catch (error) {
      if (mounted) setState(() => _previewError = '$error');
    }
  }

  Future<void> _removeThumbnail() async {
    final path = _thumbnail;
    _thumbnail = null;
    if (path == null) return;
    final file = File(path);
    if (await file.exists()) await file.parent.delete(recursive: true);
  }

  void _invalidateGit() {
    _gitGeneration++;
    _gitLoading = false;
    _gitError = null;
    _gitOutput = '';
  }

  bool _isCurrentGitRequest(int generation, String root, String view) =>
      mounted &&
      generation == _gitGeneration &&
      _root == root &&
      _gitView == view;

  Future<void> _loadGit(String view) async {
    final root = _root;
    if (root == null) return;
    final generation = ++_gitGeneration;
    setState(() {
      _gitView = view;
      _gitLoading = true;
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
      final output =
          await (widget.gitOutputForTesting?.call(root, args) ??
              DesktopFiles.boundedGitOutput(root, args));
      if (_isCurrentGitRequest(generation, root, view)) {
        setState(
          () => _gitOutput = output.trim().isEmpty ? 'No changes.' : output,
        );
      }
    } catch (error) {
      if (_isCurrentGitRequest(generation, root, view)) {
        setState(() => _gitError = '$error');
      }
    } finally {
      if (_isCurrentGitRequest(generation, root, view)) {
        setState(() => _gitLoading = false);
      }
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
            const Text(
              'Choose a project folder to browse files and Git history.',
            ),
            const SizedBox(height: 12),
            if (_browseError != null) ...[
              SizedBox(
                width: 360,
                child: Text(
                  _browseError!,
                  textAlign: TextAlign.center,
                  style: TextStyle(color: scheme.error),
                ),
              ),
              const SizedBox(height: 8),
            ],
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
              padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
              child: Row(
                children: [
                  Expanded(
                    child: SegmentedButton<bool>(
                      style: ButtonStyle(
                        visualDensity: VisualDensity.compact,
                        minimumSize: const WidgetStatePropertyAll(Size(0, 34)),
                        padding: const WidgetStatePropertyAll(
                          EdgeInsets.symmetric(horizontal: 10),
                        ),
                      ),
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
                  if (!_showGit && _selected != null) ...[
                    const SizedBox(width: 8),
                    IconButton(
                      tooltip: _previewOpen ? 'Hide preview' : 'Show preview',
                      onPressed: _togglePreview,
                      icon: Icon(
                        _previewOpen
                            ? Icons.vertical_split_outlined
                            : Icons.visibility_outlined,
                      ),
                    ),
                  ],
                ],
              ),
            ),
            Expanded(
              child: _showGit
                  ? _gitPanel(scheme)
                  : wide && _previewOpen
                  ? Row(
                      children: [
                        Expanded(flex: 5, child: files),
                        const VerticalDivider(width: 1),
                        Expanded(flex: 6, child: preview),
                      ],
                    )
                  : !wide && _previewOpen
                  ? preview
                  : files,
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
            child: Tooltip(
              message: current,
              child: Text(
                current,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.labelLarge,
              ),
            ),
          ),
          IconButton(
            tooltip: 'Choose another project folder',
            onPressed: _chooseRoot,
            icon: const Icon(Icons.folder_open_outlined),
          ),
          IconButton(
            tooltip: 'Refresh',
            onPressed: (_showGit ? _gitLoading : _loading)
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
      if (_browseError != null)
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 6, 12, 8),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  _browseError!,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: scheme.error),
                ),
              ),
              TextButton(onPressed: _refresh, child: const Text('Retry')),
            ],
          ),
        ),
      Expanded(
        child: ListView.builder(
          itemCount: _entries.length + (_truncated ? 1 : 0),
          itemBuilder: (context, index) {
            if (index == _entries.length) {
              return const SizedBox(
                height: 36,
                child: Center(child: Text('Showing first 500 entries')),
              );
            }
            final entry = _entries[index];
            return Tooltip(
              message: entry.symlink
                  ? '${entry.name} · Symlink not followed'
                  : entry.path,
              child: InkWell(
                onTap: _loading || entry.symlink ? null : () => _select(entry),
                child: Container(
                  height: 38,
                  padding: const EdgeInsets.symmetric(horizontal: 12),
                  color: entry.path == _selected
                      ? scheme.secondaryContainer.withValues(alpha: .55)
                      : null,
                  child: Row(
                    children: [
                      Icon(
                        entry.symlink
                            ? Icons.link_off
                            : entry.directory
                            ? Icons.folder_outlined
                            : Icons.insert_drive_file_outlined,
                        size: 18,
                        color: entry.symlink
                            ? scheme.onSurfaceVariant
                            : scheme.primary,
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Text(
                          entry.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(context).textTheme.bodyMedium,
                        ),
                      ),
                      if (entry.symlink)
                        Icon(
                          Icons.block,
                          size: 14,
                          color: scheme.onSurfaceVariant,
                        ),
                    ],
                  ),
                ),
              ),
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
    if (_previewLoading) {
      body = const Center(child: CircularProgressIndicator(strokeWidth: 2));
    } else if (_previewError != null) {
      body = Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.warning_amber_outlined, color: scheme.error),
            const SizedBox(height: 8),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20),
              child: Text(_previewError!, textAlign: TextAlign.center),
            ),
            const SizedBox(height: 8),
            TextButton.icon(
              onPressed: path == null
                  ? null
                  : () => _loadPreview(path, ++_previewGeneration),
              icon: const Icon(Icons.refresh, size: 16),
              label: const Text('Retry'),
            ),
          ],
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
          padding: const EdgeInsets.fromLTRB(12, 4, 8, 4),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: scheme.outlineVariant)),
          ),
          child: Row(
            children: [
              IconButton(
                tooltip: 'Hide preview',
                visualDensity: VisualDensity.compact,
                onPressed: _togglePreview,
                icon: const Icon(Icons.close, size: 18),
              ),
              Expanded(
                child: Tooltip(
                  message: path ?? '',
                  child: Text(
                    path?.split(Platform.pathSeparator).last ?? '',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.titleSmall,
                  ),
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
        Expanded(
          child: Padding(padding: const EdgeInsets.all(8), child: body),
        ),
      ],
    );
  }

  Widget _unsupportedPreview(ColorScheme scheme) => Column(
    mainAxisSize: MainAxisSize.min,
    children: [
      Icon(Icons.insert_drive_file_outlined, size: 42, color: scheme.primary),
      const SizedBox(height: 8),
      const Text('No internal preview'),
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
      if (_gitLoading) const LinearProgressIndicator(minHeight: 2),
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
                ? (_gitLoading ? 'Loading Git…' : 'No Git output.')
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
