import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';

enum NativeWorkspacePanel { browser, terminal }

const nativeWorkspacePanelChannel = MethodChannel(
  'orgbot/native_workspace_panels',
);

/// Positions a native WebKit/VTE child over this Flutter surface.
class NativeWorkspaceSurface extends StatefulWidget {
  const NativeWorkspaceSurface({
    super.key,
    required this.panel,
    this.root,
    this.uri,
    this.mediaBytes,
    this.mediaKind,
    this.mediaExtension,
    this.mediaKey,
    this.onError,
  });

  final NativeWorkspacePanel panel;
  final String? root;
  final String? uri;
  final Uint8List? mediaBytes;
  final String? mediaKind;
  final String? mediaExtension;
  final String? mediaKey;
  final ValueChanged<String>? onError;

  @override
  State<NativeWorkspaceSurface> createState() => _NativeWorkspaceSurfaceState();
}

class _NativeWorkspaceSurfaceState extends State<NativeWorkspaceSurface> {
  String? _lastRequest;
  bool _nativeSurfaceVisible = false;
  int _requestGeneration = 0;

  @override
  void didUpdateWidget(covariant NativeWorkspaceSurface oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.panel != widget.panel ||
        oldWidget.root != widget.root ||
        oldWidget.uri != widget.uri ||
        oldWidget.mediaKey != widget.mediaKey ||
        oldWidget.mediaKind != widget.mediaKind ||
        oldWidget.mediaExtension != widget.mediaExtension) {
      _lastRequest = null;
      _requestGeneration++;
    }
  }

  @override
  void dispose() {
    _requestGeneration++;
    _nativeSurfaceVisible = false;
    unawaited(_invokeHide());
    super.dispose();
  }

  Future<void> _invokeHide() async {
    try {
      await nativeWorkspacePanelChannel.invokeMethod<void>(
        widget.panel == NativeWorkspacePanel.browser
            ? (widget.mediaBytes == null
                  ? 'browser_hide'
                  : 'browser_media_hide')
            : 'terminal_hide',
      );
    } on PlatformException {
      // Native surfaces may already have been disposed with the window.
    } on MissingPluginException {
      // This widget is only mounted in the native Linux desktop build.
    }
  }

  void _hideIfVisible() {
    _lastRequest = null;
    if (!_nativeSurfaceVisible) return;
    _requestGeneration++;
    _nativeSurfaceVisible = false;
    unawaited(_invokeHide());
  }

  void _sync() {
    if (!mounted) return;
    if (ModalRoute.of(context)?.isCurrent == false) {
      _hideIfVisible();
      return;
    }
    final renderObject = context.findRenderObject();
    if (renderObject is! RenderBox || !renderObject.hasSize) return;
    RenderObject? ancestor = renderObject;
    while (ancestor != null && ancestor is! RenderView) {
      if ((ancestor is RenderOffstage && ancestor.offstage) ||
          (ancestor is RenderOpacity && ancestor.opacity == 0)) {
        _hideIfVisible();
        return;
      }
      ancestor = ancestor.parent;
    }
    final renderViews = WidgetsBinding.instance.renderViews;
    if (renderViews.isEmpty) return;
    final renderView = renderViews.first;
    final origin = renderObject.localToGlobal(
      Offset.zero,
      ancestor: renderView,
    );
    final surfaceBounds = Rect.fromLTWH(
      0,
      0,
      renderView.size.width,
      renderView.size.height,
    );
    final bounds = Rect.fromLTWH(
      origin.dx,
      origin.dy,
      renderObject.size.width,
      renderObject.size.height,
    ).intersect(surfaceBounds);
    final x = bounds.left;
    final y = bounds.top;
    final width = bounds.width;
    final height = bounds.height;
    if (bounds.isEmpty || width < 120 || height < 80) {
      _hideIfVisible();
      return;
    }
    final requestKey = [
      widget.panel.name,
      widget.root ?? '',
      widget.uri ?? '',
      widget.mediaKey ?? '',
      x.toStringAsFixed(1),
      y.toStringAsFixed(1),
      width.toStringAsFixed(1),
      height.toStringAsFixed(1),
    ].join('|');
    if (_lastRequest == requestKey) return;
    _lastRequest = requestKey;
    _nativeSurfaceVisible = true;
    final args = <String, Object?>{
      'x': x,
      'y': y,
      'width': width,
      'height': height,
      if (widget.root != null) 'root': widget.root,
      if (widget.uri != null) 'uri': widget.uri,
      if (widget.mediaBytes != null) ...{
        'bytes': widget.mediaBytes,
        'kind': widget.mediaKind,
        'extension': widget.mediaExtension,
        'media_key': widget.mediaKey,
      },
    };
    unawaited(_show(args, ++_requestGeneration));
  }

  bool _isCurrentRequest(int generation) =>
      mounted && _nativeSurfaceVisible && generation == _requestGeneration;

  Future<void> _show(Map<String, Object?> args, int generation) async {
    try {
      await nativeWorkspacePanelChannel.invokeMethod<void>(
        widget.panel == NativeWorkspacePanel.browser
            ? (widget.mediaBytes == null
                  ? 'browser_show'
                  : 'browser_media_show')
            : 'terminal_show',
        args,
      );
    } on PlatformException catch (error) {
      if (_isCurrentRequest(generation)) {
        widget.onError?.call(error.message ?? error.code);
      }
    } on MissingPluginException {
      if (_isCurrentRequest(generation)) {
        widget.onError?.call('Native Linux workspace panel is unavailable.');
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    ModalRoute.of(
      context,
    ); // Rebuild when a dialog or route covers this surface.
    WidgetsBinding.instance.addPostFrameCallback((_) => _sync());
    return ColoredBox(
      color: Theme.of(context).colorScheme.surfaceContainerLow,
      child: const SizedBox.expand(),
    );
  }
}
