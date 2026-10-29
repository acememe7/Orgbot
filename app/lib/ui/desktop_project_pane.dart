import 'dart:async';
import 'dart:io';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../workspace/desktop_files.dart';
import 'workspace_native_surface.dart';
import 'workspace_text_editor.dart';

/// Asks the project pane to show a file: a link a bot posted in the chat.
class ProjectFileRequests extends ChangeNotifier {
  String? _path;

  void show(String path) {
    _path = path;
    notifyListeners();
  }

  /// The file asked for, once; null when nothing is waiting.
  String? take() {
    final path = _path;
    _path = null;
    return path;
  }
}

class DesktopProjectPane extends StatefulWidget {
  const DesktopProjectPane({
    super.key,
    required this.projectPath,
    this.roomId,
    this.onProjectSelected,
    this.requests,
    @visibleForTesting this.initialRootForTesting,
    @visibleForTesting this.gitOutputForTesting,
    @visibleForTesting this.gitActionForTesting,
  });

  final String? projectPath;
  final String? roomId;
  final ValueChanged<String>? onProjectSelected;

  /// Files to show, asked for from outside the pane.
  final ProjectFileRequests? requests;

  @visibleForTesting
  final String? initialRootForTesting;

  @visibleForTesting
  final Future<String> Function(String root, List<String> args)?
  gitOutputForTesting;
  @visibleForTesting
  final Future<String> Function(String root, List<String> args)?
  gitActionForTesting;

  @visibleForTesting
  static bool isValidGitBranch(String value) {
    final branch = value.trim();
    return RegExp(r'^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$').hasMatch(branch) &&
        !branch.contains('..') &&
        !branch.contains('//') &&
        !branch.contains('@{') &&
        !branch.endsWith('.') &&
        !branch.endsWith('/') &&
        !branch.split('/').any((part) => part.startsWith('.'));
  }

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
  static const _audioExtensions = {
    'mp3',
    'wav',
    'ogg',
    'oga',
    'flac',
    'm4a',
    'aac',
    'opus',
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
  bool _gitActionRunning = false;
  bool _previewOpen = false;
  bool _previewLoading = false;
  bool _truncated = false;
  String _activePanel = 'files';
  final TextEditingController _webAddress = TextEditingController();
  String? _webUri;
  String? _workspaceError;
  String? _editingPath;
  final Map<String, _ProjectEditDraft> _editDrafts = {};
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
    widget.requests?.addListener(_onRequest);
    final path = widget.projectPath?.trim();
    if (path != null && path.isNotEmpty) {
      // A link clicked before the pane existed is shown once it has loaded.
      unawaited(_setRoot(path).then<void>((_) => _onRequest()));
    }
  }

  void _onRequest() {
    if (_root == null) return;
    final path = widget.requests?.take();
    if (path != null) unawaited(_reveal(path));
  }

  /// Browse to a file named in the chat and preview it.
  Future<void> _reveal(String path) async {
    final root = _root;
    if (root == null) return;
    try {
      final canonical = await File(path).resolveSymbolicLinks();
      _ensureWithinRoot(root, canonical);
      if (!mounted) return;
      setState(() {
        _activePanel = 'files';
        _current = File(canonical).parent.path;
      });
      await _refresh();
      if (!mounted) return;
      await _select(
        _ProjectEntry(
          path: canonical,
          name: canonical.split(Platform.pathSeparator).last,
          directory: false,
          symlink: false,
          regularFile: await FileSystemEntity.isFile(canonical),
        ),
      );
    } catch (error) {
      if (mounted) setState(() => _previewError = '$error');
    }
  }

  @override
  void didUpdateWidget(covariant DesktopProjectPane oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.requests != widget.requests) {
      oldWidget.requests?.removeListener(_onRequest);
      widget.requests?.addListener(_onRequest);
    }
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
    widget.requests?.removeListener(_onRequest);
    unawaited(_removeThumbnail());
    _webAddress.dispose();
    for (final draft in _editDrafts.values) {
      draft.controller.dispose();
    }
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
          !_audioExtensions.contains(ext) &&
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
      } else if (_audioExtensions.contains(ext)) {
        setState(() {
          _previewBytes = bytes;
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

  Future<void> _beginEdit(String path) async {
    final root = _root;
    if (root == null) return;
    final existing = _editDrafts[path];
    if (existing != null && !existing.loading) {
      setState(() {
        _editingPath = path;
        _previewOpen = true;
      });
      return;
    }
    if (existing != null) return;
    if (_editDrafts.length >= 8) {
      final clean = _editDrafts.entries
          .where((entry) => !entry.value.dirty && !entry.value.loading)
          .map((entry) => entry.key)
          .firstOrNull;
      if (clean == null) {
        setState(
          () => _previewError =
              'Close or save an open edit before editing another file.',
        );
        return;
      }
      _editDrafts.remove(clean)?.controller.dispose();
    }
    final draft = _ProjectEditDraft();
    _editDrafts[path] = draft;
    setState(() {
      _editingPath = path;
      _previewOpen = true;
    });
    try {
      final document = await DesktopFiles.readEditableProjectFile(root, path);
      final text = utf8.decode(document.bytes, allowMalformed: false);
      if (text.contains('\u0000')) {
        throw const FormatException('Binary files cannot be edited as text.');
      }
      if (!mounted || !identical(_editDrafts[path], draft)) return;
      draft
        ..controller.text = text
        ..original = text
        ..versionToken = document.versionToken
        ..loading = false;
      setState(() {});
    } catch (error) {
      if (!mounted || !identical(_editDrafts[path], draft)) return;
      draft
        ..loading = false
        ..error = _errorMessage(error);
      setState(() {});
    }
  }

  Future<void> _reloadEdit(String path) async {
    final root = _root;
    final draft = _editDrafts[path];
    if (root == null || draft == null || draft.saving) return;
    setState(() {
      draft
        ..loading = true
        ..error = null;
    });
    try {
      final document = await DesktopFiles.readEditableProjectFile(root, path);
      final text = utf8.decode(document.bytes, allowMalformed: false);
      if (text.contains('\u0000')) {
        throw const FormatException('Binary files cannot be edited as text.');
      }
      if (!mounted || !identical(_editDrafts[path], draft)) return;
      draft
        ..controller.value = TextEditingValue(
          text: text,
          selection: TextSelection.collapsed(offset: text.length),
        )
        ..original = text
        ..versionToken = document.versionToken
        ..loading = false;
      setState(() {});
    } catch (error) {
      if (!mounted || !identical(_editDrafts[path], draft)) return;
      draft
        ..loading = false
        ..error = _errorMessage(error);
      setState(() {});
    }
  }

  Future<void> _saveEdit(String path) async {
    final root = _root;
    final draft = _editDrafts[path];
    if (root == null || draft == null || !draft.dirty || draft.saving) return;
    final content = draft.controller.text;
    setState(() {
      draft
        ..saving = true
        ..error = null;
    });
    try {
      final token = await DesktopFiles.saveEditableProjectFile(
        root: root,
        path: path,
        bytes: Uint8List.fromList(utf8.encode(content)),
        expectedToken: draft.versionToken!,
      );
      if (!mounted || !identical(_editDrafts[path], draft)) return;
      draft
        ..versionToken = token
        ..original = content
        ..saving = false;
      setState(() {});
    } catch (error) {
      if (!mounted || !identical(_editDrafts[path], draft)) return;
      draft
        ..saving = false
        ..error = _errorMessage(error);
      setState(() {});
    }
  }

  String _errorMessage(Object error) =>
      error is PlatformException ? error.message ?? error.code : '$error';

  Future<void> _discardEdit(String path) async {
    final draft = _editDrafts[path];
    if (draft == null || !draft.dirty) {
      setState(() => _editingPath = null);
      return;
    }
    final discard = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Discard unsaved changes?'),
        content: const Text('The file has edits that have not been saved.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Keep editing'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Discard'),
          ),
        ],
      ),
    );
    if (discard == true && mounted) {
      draft.controller.dispose();
      _editDrafts.remove(path);
      setState(() => _editingPath = null);
    }
  }

  void _togglePreview() {
    if (_selected == null) return;
    setState(() => _previewOpen = !_previewOpen);
  }

  void _closeEditor() {
    setState(() => _editingPath = null);
  }

  void _selectPanel(String panel) {
    setState(() {
      _activePanel = panel;
      _workspaceError = null;
    });
    if (panel == 'git' && _gitOutput.isEmpty && _gitError == null) {
      unawaited(_loadGit('status'));
    }
  }

  Future<void> _openBrowser() async {
    var value = _webAddress.text.trim();
    if (value.isEmpty) return;
    if (!value.contains('://')) value = 'https://$value';
    final uri = Uri.tryParse(value);
    if (uri == null ||
        !{'http', 'https'}.contains(uri.scheme.toLowerCase()) ||
        uri.host.isEmpty ||
        uri.userInfo.isNotEmpty ||
        value.length > 4096) {
      setState(() => _workspaceError = 'Enter a valid HTTP or HTTPS address.');
      return;
    }
    setState(() {
      _webUri = uri.toString();
      _workspaceError = null;
      _activePanel = 'browser';
    });
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

  Future<void> _runGitAction(String action, String input) async {
    final root = _root;
    if (root == null || _gitActionRunning) return;
    final value = input.trim();
    late final List<String> args;
    switch (action) {
      case 'branch':
        if (!DesktopProjectPane.isValidGitBranch(value)) {
          setState(() => _gitError = 'Enter a valid branch name.');
          return;
        }
        args = ['switch', '-c', value];
      case 'merge':
        if (!DesktopProjectPane.isValidGitBranch(value)) {
          setState(() => _gitError = 'Enter a valid source branch name.');
          return;
        }
        args = ['merge', '--no-edit', '--no-ff', value];
      case 'commit':
        if (value.isEmpty || value.length > 5000 || value.contains('\u0000')) {
          setState(
            () => _gitError = 'Commit message must be 1–5000 characters.',
          );
          return;
        }
        args = ['commit', '-m', value];
      default:
        return;
    }
    setState(() {
      _gitActionRunning = true;
      _gitError = null;
      _gitOutput = '';
    });
    try {
      final output =
          await (widget.gitActionForTesting?.call(root, args) ??
              DesktopFiles.boundedGitOutput(root, args));
      if (!mounted || _root != root) return;
      setState(
        () => _gitOutput = output.trim().isEmpty
            ? 'Git operation completed.'
            : output,
      );
      if (widget.gitActionForTesting == null) await _refresh();
      if (mounted && _root == root) await _loadGit('status');
    } catch (error) {
      if (mounted && _root == root) setState(() => _gitError = '$error');
    } finally {
      if (mounted && _root == root) setState(() => _gitActionRunning = false);
    }
  }

  Future<void> _showGitActionDialog(String action) async {
    final input = await showDialog<String>(
      context: context,
      builder: (context) => _GitActionDialog(action: action),
    );
    if (input == null || !mounted) return;
    await _runGitAction(action, input);
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
                      emptySelectionAllowed: true,
                      selected: {
                        if (_activePanel == 'git') true,
                        if (_activePanel == 'files') false,
                      },
                      onSelectionChanged: (selection) {
                        if (selection.isEmpty) return;
                        _selectPanel(selection.first ? 'git' : 'files');
                      },
                    ),
                  ),
                  IconButton(
                    tooltip: 'Local terminal',
                    visualDensity: VisualDensity.compact,
                    onPressed: () => _selectPanel('terminal'),
                    isSelected: _activePanel == 'terminal',
                    icon: const Icon(Icons.terminal),
                  ),
                  IconButton(
                    tooltip: 'Web preview',
                    visualDensity: VisualDensity.compact,
                    onPressed: () => _selectPanel('browser'),
                    isSelected: _activePanel == 'browser',
                    icon: const Icon(Icons.language),
                  ),
                  if (_activePanel == 'files' && _selected != null) ...[
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
              child: _activePanel == 'git'
                  ? _gitPanel(scheme)
                  : _activePanel == 'terminal'
                  ? _terminalPanel(scheme, root)
                  : _activePanel == 'browser'
                  ? _browserPanel(scheme)
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
            onPressed:
                (_activePanel == 'git'
                    ? (_gitLoading || _gitActionRunning)
                    : _loading)
                ? null
                : _activePanel == 'git'
                ? () => _loadGit(_gitView)
                : _activePanel == 'files'
                ? _refresh
                : _activePanel == 'browser'
                ? () => unawaited(
                    nativeWorkspacePanelChannel.invokeMethod<void>(
                      'browser_reload',
                    ),
                  )
                : null,
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
    final draft = path == null ? null : _editDrafts[path];
    if (path != null && _editingPath == path && draft != null) {
      body = WorkspaceTextEditor(
        controller: draft.controller,
        loading: draft.loading,
        saving: draft.saving,
        error: draft.error,
        dirty: draft.dirty,
        onChanged: (_) => setState(() {}),
        onReload: () => _reloadEdit(path),
      );
    } else if (_previewLoading) {
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
    } else if ((_videoExtensions.contains(ext) ||
            _audioExtensions.contains(ext)) &&
        _previewBytes != null) {
      final kind = _videoExtensions.contains(ext) ? 'video' : 'audio';
      body = NativeWorkspaceSurface(
        panel: NativeWorkspacePanel.browser,
        mediaBytes: _previewBytes,
        mediaKind: kind,
        mediaExtension: ext,
        mediaKey: '$path:${_previewBytes!.length}:$_previewGeneration',
        onError: (error) => setState(() => _previewError = error),
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
              if (path != null && _textExtensions.contains(ext))
                if (_editingPath == path && draft != null) ...[
                  IconButton(
                    tooltip: 'Save changes',
                    onPressed: draft.dirty && !draft.saving
                        ? () => _saveEdit(path)
                        : null,
                    icon: const Icon(Icons.save_outlined, size: 18),
                  ),
                  IconButton(
                    tooltip: 'Discard changes',
                    onPressed: draft.saving ? null : () => _discardEdit(path),
                    icon: const Icon(Icons.delete_outline, size: 18),
                  ),
                  IconButton(
                    tooltip: 'Close editor',
                    onPressed: _closeEditor,
                    icon: const Icon(Icons.visibility_outlined, size: 18),
                  ),
                ] else
                  IconButton(
                    tooltip: 'Edit file',
                    onPressed: () => _beginEdit(path),
                    icon: const Icon(Icons.edit_outlined, size: 18),
                  ),
              if (path != null)
                IconButton(
                  tooltip: 'Open With',
                  onPressed: _openSelected,
                  icon: const Icon(Icons.open_in_new, size: 18),
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

  Widget _terminalPanel(ColorScheme scheme, String root) => Column(
    children: [
      Container(
        height: 38,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: scheme.outlineVariant)),
        ),
        child: Row(
          children: [
            const Expanded(
              child: Text('Local shell · runs as your Linux user'),
            ),
            IconButton(
              tooltip: 'Stop terminal process group',
              visualDensity: VisualDensity.compact,
              onPressed: () => unawaited(
                nativeWorkspacePanelChannel.invokeMethod<void>('terminal_stop'),
              ),
              icon: const Icon(Icons.stop_circle_outlined, size: 18),
            ),
          ],
        ),
      ),
      if (_workspaceError != null)
        Padding(
          padding: const EdgeInsets.all(8),
          child: Text(_workspaceError!, style: TextStyle(color: scheme.error)),
        ),
      Expanded(
        child: NativeWorkspaceSurface(
          panel: NativeWorkspacePanel.terminal,
          root: root,
          onError: (error) => setState(() => _workspaceError = error),
        ),
      ),
    ],
  );

  Widget _browserPanel(ColorScheme scheme) => Column(
    children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(10, 0, 8, 8),
        child: Row(
          children: [
            Expanded(
              child: TextField(
                controller: _webAddress,
                onSubmitted: (_) => unawaited(_openBrowser()),
                decoration: const InputDecoration(
                  isDense: true,
                  hintText: 'https://example.com',
                  prefixIcon: Icon(Icons.language, size: 18),
                ),
              ),
            ),
            const SizedBox(width: 6),
            IconButton.filledTonal(
              tooltip: 'Open address',
              visualDensity: VisualDensity.compact,
              onPressed: () => unawaited(_openBrowser()),
              icon: const Icon(Icons.arrow_forward, size: 18),
            ),
          ],
        ),
      ),
      if (_workspaceError != null)
        Padding(
          padding: const EdgeInsets.fromLTRB(10, 0, 10, 8),
          child: Text(_workspaceError!, style: TextStyle(color: scheme.error)),
        ),
      Expanded(
        child: _webUri == null
            ? Center(
                child: Text(
                  'Enter a web address to preview it here.',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              )
            : NativeWorkspaceSurface(
                panel: NativeWorkspacePanel.browser,
                uri: _webUri,
                onError: (error) => setState(() => _workspaceError = error),
              ),
      ),
    ],
  );

  Widget _gitPanel(ColorScheme scheme) => Column(
    children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(10, 0, 10, 8),
        child: Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            OutlinedButton.icon(
              onPressed: _gitActionRunning
                  ? null
                  : () => _showGitActionDialog('branch'),
              icon: const Icon(Icons.account_tree_outlined, size: 16),
              label: const Text('Branch'),
            ),
            OutlinedButton.icon(
              onPressed: _gitActionRunning
                  ? null
                  : () => _showGitActionDialog('merge'),
              icon: const Icon(Icons.merge, size: 16),
              label: const Text('Merge'),
            ),
            OutlinedButton.icon(
              onPressed: _gitActionRunning
                  ? null
                  : () => _showGitActionDialog('commit'),
              icon: const Icon(Icons.commit, size: 16),
              label: const Text('Commit staged'),
            ),
          ],
        ),
      ),
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
      if (_gitActionRunning) const LinearProgressIndicator(minHeight: 2),
      if (_gitActionRunning)
        const Padding(
          padding: EdgeInsets.all(8),
          child: Text('Running the requested local Git operation…'),
        ),
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

class _ProjectEditDraft {
  final TextEditingController controller = TextEditingController();
  String original = '';
  String? versionToken;
  String? error;
  bool loading = true;
  bool saving = false;

  bool get dirty => controller.text != original;
}

class _GitActionDialog extends StatefulWidget {
  const _GitActionDialog({required this.action});

  final String action;

  @override
  State<_GitActionDialog> createState() => _GitActionDialogState();
}

class _GitActionDialogState extends State<_GitActionDialog> {
  final TextEditingController _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final title = switch (widget.action) {
      'branch' => 'Create branch and switch',
      'merge' => 'Merge local branch',
      _ => 'Commit staged changes',
    };
    final label = switch (widget.action) {
      'branch' => 'New branch name',
      'merge' => 'Source branch',
      _ => 'Commit message',
    };
    return AlertDialog(
      title: Text(title),
      content: TextField(
        controller: _controller,
        autofocus: true,
        maxLines: widget.action == 'commit' ? 4 : 1,
        maxLength: widget.action == 'commit' ? 5000 : 100,
        decoration: InputDecoration(labelText: label),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, _controller.text),
          child: Text(
            widget.action == 'merge'
                ? 'Merge'
                : widget.action == 'branch'
                ? 'Create'
                : 'Commit staged',
          ),
        ),
      ],
    );
  }
}
