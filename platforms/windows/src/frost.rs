//! The blur behind the see-through launcher, drawn by Windows' compositor in the launcher's own
//! shape. Windows rounds a window's corners by 8 px at most, less than the launcher's, so a blur
//! from the window's backdrop either showed its corners outside the launcher's or, drawn further
//! in, left an unblurred strip around the edge. A composition visual is clipped to any rounded
//! rectangle, smoothly, so the blur fills the launcher to its edge and no further.

use windows_numerics::Vector2;
use windows::System::DispatcherQueueController;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{CompositionRoundedRectangleGeometry, Compositor, SpriteVisual};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{CreateDispatcherQueueController, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions};
use windows::core::Interface;

pub struct Frost {
    // Kept so the compositor keeps drawing. Only one queue can be made per thread, and the
    // launcher makes its blur once.
    _queue: DispatcherQueueController,
    _target: DesktopWindowTarget,
    visual: SpriteVisual,
    shape: CompositionRoundedRectangleGeometry,
    size: (i32, i32, i32),
    opacity: f32,
}

impl Frost {
    /// Fills `window` with a blur of whatever is behind it. The window must have been made with
    /// `WS_EX_NOREDIRECTIONBITMAP` and allowed the host backdrop brush.
    pub fn new(window: HWND) -> Option<Frost> {
        unsafe {
            let options = DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            };
            let queue = CreateDispatcherQueueController(options).ok()?;
            let compositor = Compositor::new().ok()?;
            let interop: ICompositorDesktopInterop = compositor.cast().ok()?;
            let target = interop.CreateDesktopWindowTarget(window, false).ok()?;
            let visual = compositor.CreateSpriteVisual().ok()?;
            let brush = compositor.CreateHostBackdropBrush().ok()?;
            visual.SetBrush(&brush).ok()?;
            let shape = compositor.CreateRoundedRectangleGeometry().ok()?;
            let clip = compositor.CreateGeometricClipWithGeometry(&shape).ok()?;
            visual.SetClip(&clip).ok()?;
            target.SetRoot(&visual).ok()?;
            Some(Frost { _queue: queue, _target: target, visual, shape, size: (0, 0, 0), opacity: 1.0 })
        }
    }

    /// Sizes the blur to the launcher, in pixels, with its corners rounded by `radius`.
    pub fn fit(&mut self, width: i32, height: i32, radius: i32) {
        if self.size == (width, height, radius) {
            return;
        }
        self.size = (width, height, radius);
        let size = Vector2 { X: width as f32, Y: height as f32 };
        let _ = self.visual.SetSize(size);
        let _ = self.shape.SetSize(size);
        let _ = self.shape.SetCornerRadius(Vector2 { X: radius as f32, Y: radius as f32 });
    }

    /// How much of the blur shows, from 0 to 1, so it fades with the launcher.
    pub fn set_opacity(&mut self, opacity: f32) {
        let opacity = opacity.clamp(0.0, 1.0);
        if (self.opacity - opacity).abs() < 0.002 {
            return;
        }
        self.opacity = opacity;
        let _ = self.visual.SetOpacity(opacity);
    }
}
