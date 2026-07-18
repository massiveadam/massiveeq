import QtQuick
import qs.Commons

Canvas {
  id: root

  property string kind: "PK"
  property real gainDb: 0
  property color lineColor: Color.mPrimary

  implicitWidth: Math.round(25 * Style.uiScaleRatio)
  implicitHeight: Math.round(18 * Style.uiScaleRatio)

  onKindChanged: requestPaint()
  onGainDbChanged: requestPaint()
  onLineColorChanged: requestPaint()
  onWidthChanged: requestPaint()
  onHeightChanged: requestPaint()

  onPaint: {
    const context = getContext("2d");
    context.clearRect(0, 0, width, height);
    context.strokeStyle = root.lineColor;
    context.lineWidth = Math.max(1.4, 1.7 * Style.uiScaleRatio);
    context.lineCap = "round";
    context.lineJoin = "round";
    const middle = height / 2;
    const top = height * 0.18;
    const bottom = height * 0.82;
    const raised = root.gainDb < 0 ? bottom : top;
    context.beginPath();
    if (kind === "LSC") {
      context.moveTo(1, raised);
      context.bezierCurveTo(width * 0.35, raised, width * 0.42, middle, width * 0.68, middle);
      context.lineTo(width - 1, middle);
    } else if (kind === "HSC") {
      context.moveTo(1, middle);
      context.lineTo(width * 0.32, middle);
      context.bezierCurveTo(width * 0.58, middle, width * 0.65, raised, width - 1, raised);
    } else if (kind === "LPQ") {
      context.moveTo(1, top);
      context.lineTo(width * 0.45, top);
      context.bezierCurveTo(width * 0.65, top, width * 0.7, bottom, width - 1, bottom);
    } else if (kind === "HPQ") {
      context.moveTo(1, bottom);
      context.bezierCurveTo(width * 0.3, bottom, width * 0.35, top, width * 0.55, top);
      context.lineTo(width - 1, top);
    } else if (kind === "BP") {
      context.moveTo(1, bottom);
      context.bezierCurveTo(width * 0.28, bottom, width * 0.32, top, width / 2, top);
      context.bezierCurveTo(width * 0.68, top, width * 0.72, bottom, width - 1, bottom);
    } else if (kind === "NO") {
      context.moveTo(1, middle);
      context.bezierCurveTo(width * 0.3, middle, width * 0.36, bottom, width / 2, bottom);
      context.bezierCurveTo(width * 0.64, bottom, width * 0.7, middle, width - 1, middle);
    } else if (kind === "AP") {
      context.moveTo(1, middle);
      context.lineTo(width - 1, middle);
    } else {
      context.moveTo(1, middle);
      context.bezierCurveTo(width * 0.3, middle, width * 0.34, raised, width / 2, raised);
      context.bezierCurveTo(width * 0.66, raised, width * 0.7, middle, width - 1, middle);
    }
    context.stroke();
  }
}
