import pytest

from PySide6.QtCore import QRect, QSize

from plain_open_pdf.gui.views.rendering.regions import geometry
from plain_open_pdf.gui.views.rendering.regions.geometry import (
    calc_base_pixels,
    calc_base_zoom,
    calc_region_plan,
)

_PAGE = QSize(4000, 6000)


def test_plan_snaps_outward_and_adds_margins() -> None:
    plan = calc_region_plan(page=_PAGE, view=QRect(500, 1000, 1000, 800))

    assert plan is not None
    # The view itself, edges moved out to multiples of 64.
    assert plan.tight == QRect(448, 960, 1088, 896)
    # 128 px at the sides and 256 px above and below, then snapped.
    assert plan.wide == QRect(320, 704, 1344, 1408)
    # 8% of the view, at most half the wide margin, and not snapped.
    assert plan.needed == QRect(436, 936, 1128, 928)


def test_vertical_margin_grows_with_a_tall_view() -> None:
    page = QSize(4000, 60000)

    plan = calc_region_plan(page=page, view=QRect(0, 20000, 1000, 2000))

    assert plan is not None
    # A quarter of 2000 px is more than the 256 px minimum.
    assert plan.wide.top() == 19456
    assert plan.wide.y() + plan.wide.height() == 22528


def test_plan_is_cut_at_the_top_left_of_the_page() -> None:
    plan = calc_region_plan(page=_PAGE, view=QRect(-100, -50, 1000, 800))

    assert plan is not None
    assert plan.tight == QRect(0, 0, 960, 768)
    assert plan.wide == QRect(0, 0, 1088, 1024)
    assert plan.needed == QRect(0, 0, 964, 814)


def test_plan_is_cut_at_the_bottom_right_of_the_page() -> None:
    plan = calc_region_plan(page=_PAGE, view=QRect(3500, 5500, 1000, 800))

    assert plan is not None
    assert plan.tight == QRect(3456, 5440, 544, 560)
    assert plan.wide == QRect(3328, 5184, 672, 816)
    assert plan.needed == QRect(3436, 5436, 564, 564)


def test_small_page_is_rendered_whole() -> None:
    assert (
        calc_region_plan(
            page=QSize(1999, 2000),
            view=QRect(0, 0, 100, 100),
        )
        is None
    )


def test_view_covering_most_of_the_page_renders_it_whole() -> None:
    page = QSize(2000, 2000)

    assert calc_region_plan(page=page, view=QRect(0, 0, 1900, 1100)) is None
    assert calc_region_plan(page=page, view=QRect(0, 0, 800, 600)) is not None


@pytest.mark.parametrize("left", [-2600, -340, 0, 900, 1900])
@pytest.mark.parametrize("top", [-3000, -1250, -400, 0, 300, 1106, 2400, 4100])
def test_choice_of_whole_page_ignores_where_the_view_is(
    left: int,
    top: int,
) -> None:
    view = QRect(left, top, 2500, 1250)

    # The view with its margins is 2756 x 1876 pixels: most of the first
    # page, less than half of the second, wherever the view is.
    assert calc_region_plan(page=QSize(1820, 2356), view=view) is None
    assert calc_region_plan(page=QSize(1820, 4000), view=view) is not None


def test_page_far_outside_the_view_has_nothing_to_render() -> None:
    plan = calc_region_plan(page=_PAGE, view=QRect(0, 7000, 1000, 800))

    assert plan is not None
    assert plan.tight.isEmpty()
    assert plan.needed.isEmpty()
    assert plan.wide.isEmpty()


def test_page_just_outside_the_view_gets_only_a_margin() -> None:
    plan = calc_region_plan(page=_PAGE, view=QRect(0, 6100, 1000, 800))

    assert plan is not None
    assert plan.tight.isEmpty()
    assert plan.needed.isEmpty()
    assert plan.wide == QRect(0, 5824, 1152, 176)


@pytest.mark.parametrize(
    "view",
    [
        QRect(0, 0, 1900, 1000),
        QRect(-300, 250, 3000, 700),
        QRect(1234, 4321, 640, 480),
        QRect(3900, 5900, 500, 500),
        QRect(10, 10, 60, 5000),
    ],
)
def test_wide_region_covers_what_its_view_needs(view: QRect) -> None:
    plan = calc_region_plan(page=_PAGE, view=view)

    assert plan is not None
    assert plan.wide.contains(plan.needed)
    assert plan.wide.contains(plan.tight)
    assert QRect(0, 0, 4000, 6000).contains(plan.wide)


def test_limits_are_read_when_the_plan_is_made(monkeypatch) -> None:
    page = QSize(400, 400)
    view = QRect(100, 100, 50, 50)
    assert calc_region_plan(page=page, view=view) is None

    monkeypatch.setattr(geometry, "REGION_MIN_PAGE_PIXELS", 1000)
    monkeypatch.setattr(geometry, "REGION_MIN_MARGIN", 10)
    monkeypatch.setattr(geometry, "REGION_SIDE_MARGIN", 10)
    monkeypatch.setattr(geometry, "REGION_GRID", 20)
    plan = calc_region_plan(page=page, view=view)

    assert plan is not None
    assert plan.tight == QRect(100, 100, 60, 60)
    assert plan.wide == QRect(80, 80, 80, 100)


def test_base_zoom_gives_about_the_base_pixel_count() -> None:
    base_zoom = calc_base_zoom(
        zoom=2.0,
        device_pixel_ratio=1.5,
        page=QSize(3000, 2000),
    )

    # 6,000,000 pixels at scale 3.0 shrink to 1,500,000 at half of it.
    assert base_zoom == pytest.approx(1.5)


def test_base_zoom_never_exceeds_the_full_scale() -> None:
    base_zoom = calc_base_zoom(
        zoom=2.0,
        device_pixel_ratio=1.5,
        page=QSize(600, 400),
    )

    assert base_zoom == pytest.approx(3.0)


def test_base_pixels_never_exceed_the_page() -> None:
    assert calc_base_pixels(QSize(3000, 2000)) == 1_500_000
    assert calc_base_pixels(QSize(600, 400)) == 240_000
