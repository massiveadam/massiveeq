import QtQuick
import QtQuick.Layouts
import Quickshell
import qs.Commons
import qs.Modules.Bar.Extras
import qs.Services.UI
import qs.Widgets

Item {
  id: root

  property var pluginApi: null
  property ShellScreen screen
  property string widgetId: ""
  property string section: ""
  property int sectionWidgetIndex: -1
  property int sectionWidgetsCount: 0

  readonly property var controller: pluginApi?.mainInstance ?? null
  readonly property var snapshot: controller?.snapshot ?? ({
                                                              "state": "unavailable"
                                                            })
  readonly property string screenName: screen?.name ?? ""
  readonly property string barPosition: Settings.getBarPositionForScreen(screenName)
  readonly property bool isVertical: barPosition === "left" || barPosition === "right"
  readonly property real capsuleHeight: Style.getCapsuleHeightForScreen(screenName)
  readonly property real contentWidth: capsuleHeight
  readonly property real contentHeight: capsuleHeight

  implicitWidth: contentWidth
  implicitHeight: contentHeight

  function stateColor() {
    switch (snapshot.state) {
    case "active":
      return Color.mPrimary;
    case "engine_off":
      return Color.mOnSurfaceVariant;
    case "unavailable":
      return Color.mError;
    default:
      return Color.mOnSurface;
    }
  }

  Rectangle {
    id: visualCapsule
    x: Style.pixelAlignCenter(parent.width, width)
    y: Style.pixelAlignCenter(parent.height, height)
    width: root.contentWidth
    height: root.contentHeight
    color: mouseArea.containsMouse ? Color.mHover : Style.capsuleColor
    radius: Style.radiusL
    border.color: Style.capsuleBorderColor
    border.width: Style.capsuleBorderWidth

    Behavior on color {
      enabled: !Color.isTransitioning
      ColorAnimation {
        duration: Style.animationFast
        easing.type: Easing.InOutQuad
      }
    }

    NIcon {
      anchors.centerIn: parent
      icon: "wave-sine"
      pointSize: Style.getBarFontSizeForScreen(root.screenName) * 1.35
      color: mouseArea.containsMouse ? Color.mOnHover : root.stateColor()

      Behavior on color {
        enabled: !Color.isTransitioning
        ColorAnimation {
          duration: Style.animationFast
          easing.type: Easing.InOutQuad
        }
      }
    }
  }

  MouseArea {
    id: mouseArea
    anchors.fill: parent
    hoverEnabled: true
    acceptedButtons: Qt.LeftButton | Qt.RightButton
    cursorShape: Qt.PointingHandCursor
    onEntered: {
      if (!pluginApi?.panelOpenScreen)
        TooltipService.show(root, "MassiveEQ — " + (controller?.stateLabel() ?? "Unavailable"), BarService.getTooltipDirection(root.screenName));
    }
    onExited: TooltipService.hide(root)
    onClicked: mouse => {
                 TooltipService.hide(root);
                 if (mouse.button === Qt.RightButton)
                   controller?.openFullApp();
                 else
                   pluginApi?.togglePanel(root.screen, root);
               }
  }
}
