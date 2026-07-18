import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import qs.Commons
import qs.Widgets

Item {
  id: root

  property var profiles: []
  property string currentKey: ""
  property string placeholder: "Choose profile"
  signal selected(string key)

  readonly property var choices: [{
      "key": "",
      "name": "Unassigned",
      "activatable": true
    }].concat((profiles || []).map(profile => ({
                                                "key": profile.id,
                                                "name": profile.name,
                                                "activatable": profile.activatable
                                              })))

  implicitHeight: combo.implicitHeight
  implicitWidth: combo.implicitWidth

  function findIndex(key) {
    for (let index = 0; index < choices.length; index++) {
      if (choices[index].key === key)
        return index;
    }
    return -1;
  }

  ComboBox {
    id: combo
    anchors.fill: parent
    model: root.choices
    currentIndex: root.findIndex(root.currentKey)
    textRole: "name"
    implicitHeight: Math.round(Style.baseWidgetSize * 1.1 * Style.uiScaleRatio)
    implicitWidth: Math.round(250 * Style.uiScaleRatio)

    onActivated: index => {
                   const choice = root.choices[index];
                   if (choice?.activatable) {
                     root.selected(choice.key);
                     Qt.callLater(() => combo.currentIndex = root.findIndex(root.currentKey));
                   }
                 }

    contentItem: NText {
      leftPadding: Style.marginL
      rightPadding: combo.indicator.width + Style.marginL
      verticalAlignment: Text.AlignVCenter
      horizontalAlignment: Text.AlignLeft
      elide: Text.ElideRight
      pointSize: Style.fontSizeM
      color: Color.mOnSurface
      text: {
        const choice = root.choices[combo.currentIndex];
        if (!choice)
          return root.placeholder;
        return choice.name + (choice.activatable ? "" : " · invalid");
      }
    }

    indicator: NIcon {
      x: combo.width - width - Style.marginM
      y: (combo.height - height) / 2
      icon: "caret-down"
      pointSize: Style.fontSizeL
    }

    background: Rectangle {
      color: Color.mSurface
      border.color: combo.activeFocus ? Color.mSecondary : Color.mOutline
      border.width: Style.borderS
      radius: Style.iRadiusM
    }

    delegate: ItemDelegate {
      required property int index
      required property var modelData
      width: combo.width
      enabled: modelData.activatable
      highlighted: combo.highlightedIndex === index

      contentItem: NText {
        text: modelData.name + (modelData.activatable ? "" : " · invalid")
        pointSize: Style.fontSizeM
        color: modelData.activatable ? Color.mOnSurface : Color.mOnSurfaceVariant
        elide: Text.ElideRight
        verticalAlignment: Text.AlignVCenter
        horizontalAlignment: Text.AlignLeft
      }

      background: Rectangle {
        color: parent.highlighted ? Color.mHover : "transparent"
        radius: Style.iRadiusS
      }
    }

    popup: Popup {
      y: combo.height + Style.marginS
      width: combo.width
      implicitHeight: Math.min(contentItem.implicitHeight + padding * 2, Math.round(260 * Style.uiScaleRatio))
      padding: Style.marginS

      contentItem: ListView {
        clip: true
        implicitHeight: contentHeight
        model: combo.popup.visible ? combo.delegateModel : null
        currentIndex: combo.highlightedIndex
        ScrollIndicator.vertical: ScrollIndicator {}
      }

      background: Rectangle {
        color: Color.mSurfaceVariant
        border.color: Color.mOutline
        border.width: Style.borderS
        radius: Style.iRadiusM
      }
    }
  }

  Connections {
    target: root
    function onCurrentKeyChanged() {
      combo.currentIndex = root.findIndex(root.currentKey);
    }
    function onChoicesChanged() {
      combo.currentIndex = root.findIndex(root.currentKey);
    }
  }
}
