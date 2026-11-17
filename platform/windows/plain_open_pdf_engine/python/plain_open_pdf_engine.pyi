# Types of the plain_open_pdf_engine Python module, which the engine's
# `python` crate builds. Keep this stub in step with `src/lib.rs`.
#
# Rectangles are (left, bottom, right, top) tuples in points, from the
# bottom left of the unrotated page.

from collections.abc import Sequence
from os import PathLike

type _Rectangle = tuple[float, float, float, float]

class PdfError(Exception): ...
class PasswordRequired(PdfError): ...  # noqa: N818 - named by the module
class WrongPassword(PasswordRequired): ...

def text_width(
    text: str, font: str = "Helvetica", size: float = 11.0
) -> float: ...

class View:
    fit: str
    left: float | None
    top: float | None
    zoom: float | None
    area: _Rectangle | None
    def __init__(
        self,
        fit: str,
        left: float | None = None,
        top: float | None = None,
        zoom: float | None = None,
        area: _Rectangle | None = None,
    ) -> None: ...

class Page:
    media_box: _Rectangle
    crop_box: _Rectangle
    rotation: int

class OutlineItem:
    level: int
    title: str
    page: int | None
    view: View | None

class Link:
    rectangle: _Rectangle
    kind: str
    page: int | None
    uri: str | None
    file: str | None
    name: str | None
    view: View | None

class Attachment:
    name: str
    file_name: str | None
    description: str | None
    stream: int | None

type _NewOutlineItem = (
    OutlineItem
    | tuple[int, str, int | None]
    | tuple[int, str, int | None, View | None]
)

class Document:
    def __init__(self, data: bytes, password: str | None = None) -> None: ...
    @staticmethod
    def new() -> Document: ...
    @staticmethod
    def open(
        path: str | PathLike[str], password: str | None = None
    ) -> Document: ...
    @property
    def page_count(self) -> int: ...
    def __len__(self) -> int: ...
    @property
    def version(self) -> str: ...
    @property
    def pages(self) -> list[Page]: ...
    def page(self, page_index: int) -> Page: ...
    @property
    def encryption(self) -> str | None: ...
    @property
    def permissions(self) -> dict[str, bool] | None: ...
    def information(self) -> dict[str, str]: ...
    def information_entries(self) -> dict[str, str]: ...
    def outline(self) -> list[OutlineItem]: ...
    def links(self, page_index: int) -> list[Link]: ...
    def named_destination(
        self, name: str
    ) -> tuple[int, View | None] | None: ...
    def attachments(self) -> list[Attachment]: ...
    def attachment_data(self, name: str) -> bytes | None: ...
    def page_text(self, page_index: int) -> str: ...
    def page_glyphs(
        self, page_index: int
    ) -> list[tuple[str, float, float, float, float, float]]: ...
    def page_words(
        self, page_index: int
    ) -> list[tuple[float, float, float, float, str, int, int, int]]: ...
    def page_spans(
        self, page_index: int
    ) -> list[
        tuple[
            str,
            _Rectangle,
            tuple[float, float],
            str,
            float,
            tuple[int, int, int],
        ]
    ]: ...
    def to_view(
        self, page_index: int, x: float, y: float
    ) -> tuple[float, float] | None: ...
    def from_view(
        self, page_index: int, x: float, y: float
    ) -> tuple[float, float] | None: ...
    def view_size(self, page_index: int) -> tuple[float, float] | None: ...
    def search_page(
        self, page_index: int, needle: str
    ) -> list[list[_Rectangle]]: ...
    def set_fallback_font(self, data: bytes) -> bool: ...
    def render_page(
        self, page_index: int, scale: float = 1.0, mode: str = "RGBA"
    ) -> tuple[int, int, bytes]: ...
    def render_page_region(
        self,
        page_index: int,
        scale: float,
        left: int,
        top: int,
        width: int,
        height: int,
        mode: str = "RGBA",
    ) -> tuple[int, int, bytes]: ...
    def render_page_png(self, page_index: int, scale: float = 1.0) -> bytes: ...
    def page_content(self, page_index: int) -> bytes: ...
    def stream(self, number: int, generation: int = 0) -> bytes | None: ...
    def set_outline(self, items: Sequence[_NewOutlineItem]) -> None: ...
    def set_page_rotation(self, page_index: int, degrees: int) -> None: ...
    def insert_blank_page(
        self, page_index: int, width: float, height: float
    ) -> None: ...
    def break_page(self, page_index: int, height: float) -> None: ...
    def select_pages(self, indexes: Sequence[int]) -> None: ...
    def set_information(self, key: str, value: str | None = None) -> None: ...
    def add_link(
        self,
        page_index: int,
        rectangle: _Rectangle,
        kind: str,
        page: int | None = None,
        uri: str | None = None,
        file: str | None = None,
        name: str | None = None,
        view: View | None = None,
    ) -> None: ...
    def redact(
        self,
        page_index: int,
        areas: Sequence[_Rectangle],
        fill: tuple[float, float, float] | None = (1.0, 1.0, 1.0),
    ) -> dict[str, int]: ...
    def insert_text(
        self,
        page_index: int,
        x: float,
        y: float,
        text: str,
        font: str = "Helvetica",
        size: float = 11.0,
        color: tuple[float, float, float] = (0.0, 0.0, 0.0),
    ) -> float: ...
    def add_attachment(
        self,
        name: str,
        data: bytes,
        file_name: str | None = None,
        description: str | None = None,
    ) -> None: ...
    def remove_attachment(self, name: str) -> bool: ...
    def xmp_metadata(self) -> str | None: ...
    def set_xmp_metadata(self, text: str | None = None) -> None: ...
    def save(self) -> bytes: ...
    def save_decrypted(self) -> bytes: ...
    def save_encrypted(
        self,
        user_password: str = "",
        owner_password: str = "",
        permissions: Sequence[str] | None = None,
    ) -> bytes: ...
