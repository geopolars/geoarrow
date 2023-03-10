use crate::algorithm::bounding_rect::bounding_rect_linestring;
use crate::geo_traits::LineStringTrait;
use crate::Point;
use arrow2::buffer::Buffer;
use arrow2::offset::OffsetsBuffer;
use rstar::{RTreeObject, AABB};

use super::iterator::LineStringIterator;

/// An Arrow equivalent of a LineString
#[derive(Debug, Clone)]
pub struct LineString<'a> {
    /// Buffer of x coordinates
    pub x: &'a Buffer<f64>,

    /// Buffer of y coordinates
    pub y: &'a Buffer<f64>,

    /// Offsets into the coordinate array where each geometry starts
    pub geom_offsets: &'a OffsetsBuffer<i64>,

    pub geom_index: usize,
}

impl<'a> LineStringTrait<'a> for LineString<'a> {
    type ItemType = Point<'a>;
    type Iter = LineStringIterator<'a>;

    fn points(&'a self) -> Self::Iter {
        LineStringIterator::new(self)
    }

    fn num_points(&self) -> usize {
        let (start, end) = self.geom_offsets.start_end(self.geom_index);
        end - start
    }

    fn point(&'a self, i: usize) -> Option<Self::ItemType> {
        let (start, end) = self.geom_offsets.start_end(self.geom_index);
        if i > (end - start) {
            return None;
        }

        let point = Point {
            x: self.x,
            y: self.y,
            geom_index: start + i,
        };
        Some(point)
    }
}

impl From<LineString<'_>> for geo::LineString {
    fn from(value: LineString<'_>) -> Self {
        (&value).into()
    }
}

impl From<&LineString<'_>> for geo::LineString {
    fn from(value: &LineString<'_>) -> Self {
        let (start_idx, end_idx) = value.geom_offsets.start_end(value.geom_index);
        let mut coords: Vec<geo::Coord> = Vec::with_capacity(end_idx - start_idx);

        for i in start_idx..end_idx {
            coords.push(geo::Coord {
                x: value.x[i],
                y: value.y[i],
            })
        }

        geo::LineString::new(coords)
    }
}

impl From<LineString<'_>> for geo::Geometry {
    fn from(value: LineString<'_>) -> Self {
        geo::Geometry::LineString(value.into())
    }
}

impl RTreeObject for LineString<'_> {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        let (lower, upper) = bounding_rect_linestring(self);
        AABB::from_corners(lower, upper)
    }
}
