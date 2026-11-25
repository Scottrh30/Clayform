//! Orbit camera.

use glam::{Mat4, Vec2, Vec3, Vec4};

pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub fov_y: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { target: Vec3::ZERO, yaw: 0.35, pitch: 0.25, dist: 3.6, fov_y: 40f32.to_radians() }
    }
}

impl Camera {
    /// (target, yaw, pitch, distance), for saving.
    pub fn clone_state(&self) -> ([f32; 3], f32, f32, f32) {
        (self.target.to_array(), self.yaw, self.pitch, self.dist)
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * sy, sp, cp * cy) * self.dist
    }

    pub fn forward(&self) -> Vec3 {
        (self.target - self.eye()).normalize()
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect.max(1e-3), self.dist * 0.01, self.dist * 100.0)
    }

    pub fn orbit(&mut self, d: Vec2) {
        self.yaw -= d.x * 0.008;
        self.pitch = (self.pitch + d.y * 0.008).clamp(-1.55, 1.55);
    }

    /// `d` in points, `h` the viewport height in points.
    pub fn pan(&mut self, d: Vec2, h: f32) {
        let scale = 2.0 * self.dist * (self.fov_y * 0.5).tan() / h.max(1.0);
        let f = self.forward();
        let right = f.cross(Vec3::Y).normalize();
        let up = right.cross(f);
        self.target += (-right * d.x + up * d.y) * scale;
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.dist = (self.dist * (-scroll * 0.0015).exp()).clamp(0.05, 200.0);
    }

    /// World-space ray through a viewport point (points, origin top-left).
    pub fn ray(&self, p: Vec2, size: Vec2) -> (Vec3, Vec3) {
        let ndc = Vec2::new(2.0 * p.x / size.x - 1.0, 1.0 - 2.0 * p.y / size.y);
        let inv = (self.proj(size.x / size.y) * self.view()).inverse();
        let a = inv * Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
        let b = inv * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let a = a.truncate() / a.w;
        let b = b.truncate() / b.w;
        (self.eye(), (b - a).normalize())
    }

    /// World units covered by one point on screen at the depth of `at`.
    pub fn world_per_point(&self, at: Vec3, h: f32) -> f32 {
        let depth = (at - self.eye()).dot(self.forward()).max(1e-4);
        2.0 * depth * (self.fov_y * 0.5).tan() / h.max(1.0)
    }

    pub fn frame(&mut self, min: Vec3, max: Vec3) {
        self.target = (min + max) * 0.5;
        let r = ((max - min) * 0.5).length().max(1e-3);
        self.dist = r / (self.fov_y * 0.5).sin() * 1.1;
    }
}
